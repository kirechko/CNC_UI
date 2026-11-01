//! SQLite → `rosetta::macro_engine::MacroRegistry` bridge (T26 plan §3.2 —
//! "глазная игла #2").
//!
//! The MACROS panel stores `user_macros` rows with a **flat G-code body**.
//! Rosetta executes `MacroProgram`s (IR of statements). The bridge converts
//! one into the other without a destructive migration:
//!
//! * `ensure_schema` adds a nullable `rosetta_yaml TEXT` column idempotently;
//!   when it carries text, that YAML *is* the canonical Macro IR source of
//!   truth (ТЗ №2 §3: "stored as IR", YAML being its serialization);
//! * when the column is empty, the flat body becomes a straight-line program:
//!   one `Stmt::GCode` per non-empty line, `; …` lines become `Stmt::Comment`,
//!   and interpolation braces are escaped (`{` → `{{`) so the template engine
//!   passes the text through verbatim (ТЗ №1 principle 4 — a body must never
//!   silently rename into a variable lookup).
//!
//! The registry is rebuilt lazily for every macro run (the panel caps users
//! at 200 macros), which keeps the conversion simple and always consistent
//! with the last panel save.

use rusqlite::{Connection, OptionalExtension};

use rosetta::error::RosettaResult;
use rosetta::io::{MacroFrontend, YamlFrontend};
use rosetta::ir::capability::{
    CapabilityMatrix, CapabilityRegistry, ConfirmPolicy, Resolution, ResolutionRule,
};
use rosetta::ir::macro_ir::{MacroClass, MacroMeta, MacroProgram, Stmt};
use rosetta::ir::toolpath::Code;
use rosetta::macro_engine::registry::MacroRegistry;

use super::backend::backend_from_key;

/// Adds the `rosetta_yaml` column to `user_macros` when missing.
///
/// Idempotent and safe on every startup: the column is nullable with no
/// default, so existing rows and the panel's INSERT/UPDATE statements (which
/// name their columns explicitly) keep working untouched.
pub fn ensure_schema(connection: &Connection) -> Result<(), String> {
    let has_column: bool = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('user_macros') WHERE name = 'rosetta_yaml'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| format!("cannot inspect user_macros schema: {error}"))?
        .unwrap_or(0)
        > 0;
    if has_column {
        return Ok(());
    }
    connection
        .execute("ALTER TABLE user_macros ADD COLUMN rosetta_yaml TEXT", [])
        .map_err(|error| format!("cannot add rosetta_yaml column: {error}"))?;
    Ok(())
}

/// Escapes template interpolation so a flat G-code line runs verbatim.
///
/// The engine's `interpolate` treats `{name}` as a variable reference and
/// `{{`/`}}` as literal braces; every brace in a legacy body therefore has to
/// be doubled before it enters `Stmt::GCode { template }`.
pub fn escape_template(line: &str) -> String {
    line.replace('{', "{{").replace('}', "}}")
}

/// Builds the straight-line `MacroProgram` projection of a flat G-code body.
///
/// The program is `reactive`: it streams its steps through the host runtime
/// exactly like the console does today, keeping live status reads available.
pub fn program_from_flat_body(id: &str, name: &str, body: &str) -> MacroProgram {
    let mut statements = Vec::new();
    for raw in body.lines() {
        let line = raw.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        if line.trim_start().starts_with(';') {
            statements.push(Stmt::Comment {
                text: line.to_owned(),
            });
        } else {
            statements.push(Stmt::GCode {
                template: escape_template(line),
            });
        }
    }
    MacroProgram {
        meta: MacroMeta {
            id: id.to_owned(),
            name: name.to_owned(),
            icon: None,
            category: None,
            class: MacroClass::Reactive,
            require_confirm: false,
        },
        params: Vec::new(),
        locals: Vec::new(),
        subs: Vec::new(),
        triggers: Vec::new(),
        body: statements,
    }
}

/// One `user_macros` row lifted into the engine's registry view.
#[derive(Debug, Clone)]
pub struct StoredMacro {
    /// Stable panel id (also the registry id).
    pub id: String,
    /// Human-readable name from the panel.
    pub name: String,
    /// Flat G-code body (fallback source when no YAML is attached).
    pub body: String,
    /// Optional canonical Macro IR YAML authored for Rosetta execution.
    pub rosetta_yaml: Option<String>,
}

/// Reads every macro row (id/name/body/optional YAML). The `rosetta_yaml`
/// column may be absent in databases created before the migration ran, so the
/// query falls back to the pre-migration shape.
pub fn list_stored(connection: &Connection) -> Result<Vec<StoredMacro>, String> {
    let has_yaml = connection
        .query_row(
            "SELECT COUNT(*) FROM pragma_table_info('user_macros') WHERE name = 'rosetta_yaml'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .unwrap_or(0)
        > 0;
    let sql = if has_yaml {
        "SELECT id, name, body, rosetta_yaml FROM user_macros ORDER BY name COLLATE NOCASE, id"
    } else {
        "SELECT id, name, body, NULL FROM user_macros ORDER BY name COLLATE NOCASE, id"
    };
    let mut statement = connection.prepare(sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map([], |row| {
            Ok(StoredMacro {
                id: row.get(0)?,
                name: row.get(1)?,
                body: row.get(2)?,
                rosetta_yaml: row.get(3)?,
            })
        })
        .map_err(|error| error.to_string())?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())
}

/// Parses one stored macro into IR: canonical YAML when present, otherwise
/// the flat-body projection. YAML parse failures are returned to the caller
/// (the run endpoint reports them) rather than silently falling back — an
/// authored program that no longer parses must not start executing old text.
pub fn program_of(stored: &StoredMacro) -> RosettaResult<MacroProgram> {
    match stored
        .rosetta_yaml
        .as_deref()
        .filter(|text| !text.trim().is_empty())
    {
        Some(yaml) => YamlFrontend.parse_macro(yaml),
        None => {
            let program = program_from_flat_body(&stored.id, &stored.name, &stored.body);
            program.validate().map(|()| program)
        }
    }
}

/// Assembles the engine registry from the database. Duplicate ids and invalid
/// programs abort with a descriptive error (panel saves are validated, so a
/// failure here means a corrupted or externally edited database).
pub fn build_registry(connection: &Connection) -> Result<MacroRegistry, String> {
    let mut registry = MacroRegistry::new();
    for stored in list_stored(connection)? {
        let program =
            program_of(&stored).map_err(|error| format!("macro \"{}\": {error}", stored.id))?;
        registry
            .upsert(program)
            .map_err(|error| format!("macro \"{}\": {error}", stored.id))?;
    }
    Ok(registry)
}

/// One row of the `rosetta_settings` substitution presets (the schema the
/// seeding module writes; values are JSON of `SubstitutionPreset`).
#[derive(Debug, serde::Deserialize)]
struct StoredPreset {
    backend: String,
    word: String,
    resolution: String,
    macro_id: String,
    /// Recorded but unused today (`simple-moves` is the only strategy);
    /// deserialized so future fallback policies do not reject old rows.
    #[serde(default, rename = "fallback")]
    _fallback: String,
}

/// Layers the operator-editable `rosetta_settings` substitution presets over
/// the built-in matrix, so the database remains the single source of truth
/// for cycle substitution (seed rows: `substitute:grbl:G83` → macro
/// `g83_peck_drill`). Rows of unknown shape are skipped silently: a corrupt
/// preset must not break validation for every other code.
pub fn matrix_with_presets(connection: &Connection) -> CapabilityMatrix {
    let mut matrix = CapabilityMatrix::new();
    let mut statement = match connection
        .prepare("SELECT key, value FROM rosetta_settings WHERE key LIKE 'substitute:%'")
    {
        Ok(statement) => statement,
        // The table may not exist yet on a brand-new database.
        Err(_) => return matrix,
    };
    let rows = match statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    }) {
        Ok(rows) => rows,
        Err(_) => return matrix,
    };
    for (key, value) in rows.flatten() {
        // key = "substitute:<backend>:<word>"; the payload repeats both, so
        // the key prefix is the authoritative backend token and the payload
        // is trusted only when it agrees with the key (protects against a
        // preset edited into an inconsistent state).
        let key_backend = key.split(':').nth(1).unwrap_or("");
        let preset: StoredPreset = match serde_json::from_str(&value) {
            Ok(preset) => preset,
            Err(_) => continue,
        };
        let Some(backend) = backend_from_key(key_backend).filter(|_| key_backend == preset.backend)
        else {
            continue;
        };
        let Some(code) = Code::parse(&preset.word) else {
            continue;
        };
        if preset.resolution != "substitute" {
            continue;
        }
        matrix.register_resolution(ResolutionRule {
            backend,
            code,
            resolution: Resolution::SubstituteMacro(preset.macro_id),
            confirm: ConfirmPolicy::Auto,
            note: Some("substitution preset from rosetta_settings".to_owned()),
        });
    }
    matrix
}

/// Unit tests for the flat-body conversion and the YAML preference rule.
#[cfg(test)]
mod tests {
    use super::*;
    use rosetta::error::RosettaError;
    use rusqlite::Connection;

    /// Creates a fresh macro table in the pre-migration shape.
    fn test_connection() -> Connection {
        let connection = Connection::open_in_memory().expect("in-memory db");
        connection
            .execute_batch(
                "CREATE TABLE user_macros (
                     id TEXT PRIMARY KEY,
                     name TEXT NOT NULL,
                     description TEXT NOT NULL,
                     body TEXT NOT NULL,
                     invocations TEXT NOT NULL
                 );",
            )
            .expect("create table");
        connection
    }

    #[test]
    fn ensure_schema_adds_column_idempotently() {
        let connection = test_connection();
        ensure_schema(&connection).expect("first add");
        ensure_schema(&connection).expect("second add is a no-op");
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM pragma_table_info('user_macros') WHERE name='rosetta_yaml'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn flat_body_becomes_escaped_gcode_statements() {
        let program = program_from_flat_body(
            "m",
            "M",
            "; comment line\nG0 X1\nG4 P{500}\n\nG1 Z-2 F100\n",
        );
        assert_eq!(program.meta.class, MacroClass::Reactive);
        // Blank lines drop; comment lines become Stmt::Comment; braces escape.
        assert_eq!(program.body.len(), 4);
        assert!(matches!(&program.body[0], Stmt::Comment { text } if text == "; comment line"));
        assert!(matches!(&program.body[2], Stmt::GCode { template } if template == "G4 P{{500}}"));
    }

    #[test]
    fn registry_prefers_yaml_and_reports_bad_yaml() {
        let connection = test_connection();
        ensure_schema(&connection).unwrap();
        connection
            .execute(
                "INSERT INTO user_macros (id, name, description, body, invocations, rosetta_yaml)
                 VALUES ('good', 'Good', '', 'G0 X0', '[]',
                         'meta: { id: good, name: Good }\nbody:\n  - !gcode { template: \"G0 X5\" }\n')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO user_macros (id, name, description, body, invocations)
                 VALUES ('flat', 'Flat', '', 'G1 Y1\n', '[]')",
                [],
            )
            .unwrap();
        let registry = build_registry(&connection).expect("registry");
        let good = registry.get(&"good".to_string()).expect("good program");
        assert_eq!(good.body.len(), 1);
        if let Stmt::GCode { template } = &good.body[0] {
            assert_eq!(template, "G0 X5");
        } else {
            panic!("expected gcode statement");
        }
        let flat = registry.get(&"flat".to_string()).expect("flat program");
        assert_eq!(flat.body.len(), 1);
    }

    #[test]
    fn unknown_yaml_shape_fails_not_falls_back() {
        let stored = StoredMacro {
            id: "x".into(),
            name: "X".into(),
            body: "G0 X1".into(),
            rosetta_yaml: Some("not: a: valid: yaml: program".into()),
        };
        assert!(matches!(
            program_of(&stored),
            Err(RosettaError::Parse { .. }) | Err(RosettaError::Validation { .. })
        ));
    }
}
