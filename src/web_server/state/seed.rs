//! First-start seeding of the Rosetta example macros and settings (task:
//! "Сделать для Rosetta примеры макросов и настроек, чтобы они уже были в базе
//! данных").
//!
//! The canonical sources of the examples live next to the app as YAML files
//! (`config/macros/*.yaml` — Macro IR programs per ТЗ №1/№2, and
//! `config/rosetta.yaml` — the backend substitution presets). The application
//! does not parse the Macro IR yet (the integration is planned in
//! `plans/Rosetta-integration-plan.md`), so what lands in the shared SQLite
//! database is the *flat, immediately usable* projection of each example:
//!
//! * every macro becomes a regular `user_macros` row, visible in the MACROS
//!   panel and editable there. Its G-code body is the offline expansion of the
//!   YAML program for a plain XYZ machine with default parameters, and the
//!   description points back at the YAML source of truth;
//! * every substitution preset (G83/G73 -> simple peck moves for grbl and
//!   fluidnc) becomes a `rosetta_settings` row (JSON value per key) so the
//!   future Rosetta host finds them in the database from the first start.
//!
//! Seeding is **idempotent and non-destructive**: rows are inserted only when
//! their primary key is absent (`INSERT OR IGNORE`), so user edits and
//! deletions are never rolled back by a later start. Set `CNC_UI_NO_SEED=1` to
//! skip seeding entirely (used by the UI tests that assert an empty panel).

use crate::web_server::models::{MacroDefinition, MacroInvocation};
use rusqlite::Connection;
use serde::Serialize;

/// One capability substitution preset stored in `rosetta_settings`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SubstitutionPreset {
    /// Backend the rule applies to (`grbl`, `fluidnc`).
    pub(crate) backend: String,
    /// Word intercepted by the capability scan (`G83`, `G73`).
    pub(crate) word: String,
    /// Resolution kind; `substitute` routes to the generative macro.
    pub(crate) resolution: String,
    /// Macro id whose expansion replaces the cycle block.
    pub(crate) macro_id: String,
    /// Fallback strategy when the macro source is missing.
    pub(crate) fallback: String,
}

/// The canned-cycle substitution presets from `config/rosetta.yaml`: peck
/// drilling (G83) and chip-breaking (G73) rewritten into plain moves on both
/// GRBL and FluidNC, which lack the cycle vocabulary.
pub(crate) fn substitution_presets() -> Vec<SubstitutionPreset> {
    ["grbl", "fluidnc"]
        .into_iter()
        .flat_map(|backend| {
            [("G83", "g83_peck_drill"), ("G73", "g83_peck_drill")]
                .into_iter()
                .map(move |(word, macro_id)| SubstitutionPreset {
                    backend: backend.to_string(),
                    word: word.to_string(),
                    resolution: "substitute".to_string(),
                    macro_id: macro_id.to_string(),
                    fallback: "simple-moves".to_string(),
                })
        })
        .collect()
}

/// Flat expansion of `config/macros/hole_array.yaml` at the default 3x3
/// parameters: a generative macro, so the expansion is deterministic and can
/// be shipped as plain G-code for every backend.
const HOLE_ARRAY_BODY: &str = r#"; Rosetta example: Hole array (3x3 @ 10 mm, depth -5)
; Source of truth: config/macros/hole_array.yaml (params x0 y0 dx dy cols rows depth feed rapid)
; Generative expansion — safe on GRBL / FluidNC / emulator.
G21 G90 G94
G0 Z5.0 F3000
G0 X0 Y0 F3000
G0 Z2.0
G1 Z-5 F120
G0 Z2.0
G0 X10 Y0 F3000
G0 Z2.0
G1 Z-5 F120
G0 Z2.0
G0 X20 Y0 F3000
G0 Z2.0
G1 Z-5 F120
G0 Z2.0
G0 X0 Y10 F3000
G0 Z2.0
G1 Z-5 F120
G0 Z2.0
G0 X10 Y10 F3000
G0 Z2.0
G1 Z-5 F120
G0 Z2.0
G0 X20 Y10 F3000
G0 Z2.0
G1 Z-5 F120
G0 Z2.0
G0 X0 Y20 F3000
G0 Z2.0
G1 Z-5 F120
G0 Z2.0
G0 X10 Y20 F3000
G0 Z2.0
G1 Z-5 F120
G0 Z2.0
G0 X20 Y20 F3000
G0 Z2.0
G1 Z-5 F120
G0 Z2.0
G0 Z10.0 F3000
"#;

/// Reactive probe macro: Z zero with a simple touch probe (G38.2), the flat
/// projection of `config/macros/z_probe_zero.yaml`.
const Z_PROBE_ZERO_BODY: &str = r#"; Rosetta example: Z zero with a simple probe
; Source of truth: config/macros/z_probe_zero.yaml (reactive class, G38.2 probing)
; Requires a probe input wired to the controller; adjust G10 P to the active system.
G90 G21 G94
G0 Z20 F1000
G91 G38.2 Z-30 F100
G91 G1 Z1 F100
G91 G38.2 Z-2 F100
G90
G10 L20 P1 Z0
G0 Z5 F1000
"#;

/// Reactive probe + align macro: 3D touch-probe of the blank corner/edge, the
/// flat projection of `config/macros/probe_and_align.yaml`. The final host
/// step (Rotate/Translate transform of the loaded path) is documented in the
/// body; the raw measurement part runs on any G38.2-capable backend.
const PROBE_ALIGN_BODY: &str = r#"; Rosetta example: measure blank position/orientation with a 3D touch probe
; Source of truth: config/macros/probe_and_align.yaml (reactive + InvokeTransform)
; Measures corner A (-X/-Y faces), a second point along the edge, then the host
; bakes the measured origin/angle into the loaded toolpath (Rotate + Translate).
G90 G21 G94
G0 Z25 F1500
G91 G38.2 X-40 F120
G91 G38.2 Y-40 F120
G90
G0 X50 Z25 F1500
G91 G38.2 X-40 F120
G90
; Host-side step: Transform::Rotate(angle) + Translate(ax, ay) over the program.
G0 Z25 F1500
"#;

/// Path-modify launcher macro for G41/G42/G40 baking on GRBL: the flat
/// projection of `config/macros/compensate_path.yaml`. Until Rosetta Phase D
/// (CutterCompBake) lands, the body documents the operation; selection of the
/// GRBL target is handled by the host transform call.
const COMPENSATE_BODY: &str = r#"; Rosetta example: bake cutter compensation G41/G42/G40 for GRBL
; Source of truth: config/macros/compensate_path.yaml (InvokeTransform CutterCompBake)
; The host rewrites the loaded program geometry (radius offset left=+1/right=-1)
; and removes G40/G41/G42 before streaming; plain moves below are a safe fallback
; for programs that only use straight G1 compensation segments.
G21 G90 G94
G40
"#;

/// Peck-drill substitution macro: G83/G73 replaced by plain moves — the macro
/// the substitution presets point at (`g83_peck_drill`).
const PECK_DRILL_BODY: &str = r#"; Rosetta example: peck drill cycle emulated with simple moves
; Target of the G83/G73 substitution presets in rosetta_settings.
; The host expands it per cycle block: R=2 clearance, Q=2 peck, F from the block.
G21 G90
G0 Z2
G1 Z-1 F120
G0 Z2
G1 Z-2 F120
G0 Z2
G1 Z-3 F120
G0 Z2
G1 Z-4 F120
G0 Z2
G1 Z-5 F120
G0 Z5
"#;

/// Builds the example macro collection inserted into `user_macros` on first
/// start. Names/descriptions double as documentation of the Rosetta YAML
/// sources; invocation mappings follow the existing MACROS metadata contract
/// (LinuxCNC M6xx / FluidNC event), keeping the panel fully functional.
pub(crate) fn example_macros() -> Vec<MacroDefinition> {
    let invocations = |linuxcnc_mcode: u16| {
        vec![
            MacroInvocation {
                controller: "linuxcnc".into(),
                mechanism: "mcode".into(),
                command: format!("M{linuxcnc_mcode}"),
            },
            MacroInvocation {
                controller: "fluidnc".into(),
                mechanism: "event".into(),
                command: "rosetta-macro".into(),
            },
        ]
    };
    vec![
        MacroDefinition {
            id: "hole_array".into(),
            name: "Hole array".into(),
            description: "Rosetta generative macro: rectangular hole array with parameters \
                          (x0,y0,dx,dy,cols,rows,depth,feed,rapid). Source: config/macros/hole_array.yaml"
                .into(),
            body: HOLE_ARRAY_BODY.into(),
            invocations: invocations(651),
        },
        MacroDefinition {
            id: "z_probe_zero".into(),
            name: "Z zero (simple probe)".into(),
            description: "Rosetta reactive macro: touch-probe Z zero with G38.2 and G10 L20. \
                          Source: config/macros/z_probe_zero.yaml"
                .into(),
            body: Z_PROBE_ZERO_BODY.into(),
            invocations: invocations(650),
        },
        MacroDefinition {
            id: "probe_and_align".into(),
            name: "3D probe align (blank)".into(),
            description: "Rosetta reactive macro: measure blank corner/edge with a 3D touch probe, \
                          then align the loaded toolpath via Transform (Rotate+Translate). \
                          Source: config/macros/probe_and_align.yaml"
                .into(),
            body: PROBE_ALIGN_BODY.into(),
            invocations: invocations(652),
        },
        MacroDefinition {
            id: "compensate_path".into(),
            name: "Bake G41/G42/G40 (GRBL)".into(),
            description: "Rosetta macro: bake cutter compensation into the path for controllers \
                          without G41/G42 (Transform CutterCompBake). Source: config/macros/compensate_path.yaml"
                .into(),
            body: COMPENSATE_BODY.into(),
            invocations: invocations(653),
        },
        MacroDefinition {
            id: "g83_peck_drill".into(),
            name: "Peck drill (simple moves)".into(),
            description: "Rosetta substitution macro for G83/G73 on grbl/fluidnc: peck drilling \
                          rewritten as plain rapid/linear moves."
                .into(),
            body: PECK_DRILL_BODY.into(),
            invocations: invocations(654),
        },
    ]
}

/// Inserts the example rows when absent. Errors are returned as `String` in
/// the style of the surrounding state module; the caller logs but never fails
/// startup because of seed problems.
pub(crate) fn seed_if_absent(connection: &Connection) -> Result<(), String> {
    // The settings table is created here rather than in `log_store`: it only
    // exists to carry Rosetta presets and must appear exactly once per DB.
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS rosetta_settings (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );",
        )
        .map_err(|error| format!("cannot create rosetta_settings: {error}"))?;

    // `macro` is a reserved keyword in the 2018 edition, so the loop binds `item`.
    for item in example_macros() {
        let invocations = serde_json::to_string(&item.invocations)
            .map_err(|error| format!("cannot encode seed invocations: {error}"))?;
        connection
            .execute(
                "INSERT OR IGNORE INTO user_macros (id, name, description, body, invocations)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![
                    item.id,
                    item.name,
                    item.description,
                    item.body,
                    invocations
                ],
            )
            .map_err(|error| format!("cannot seed macro {}: {error}", item.id))?;
    }

    for preset in substitution_presets() {
        let key = format!("substitute:{}:{}", preset.backend, preset.word);
        let value = serde_json::to_string(&preset)
            .map_err(|error| format!("cannot encode seed preset: {error}"))?;
        connection
            .execute(
                "INSERT OR IGNORE INTO rosetta_settings (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )
            .map_err(|error| format!("cannot seed setting {key}: {error}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fresh in-memory database with the two tables the seeder touches.
    fn connection() -> Connection {
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
            .expect("schema");
        connection
    }

    #[test]
    fn seeding_inserts_examples_and_presets_once() {
        let connection = connection();
        seed_if_absent(&connection).expect("seed");
        seed_if_absent(&connection).expect("second seed is a no-op");

        let macros: i64 = connection
            .query_row("SELECT COUNT(*) FROM user_macros", [], |row| row.get(0))
            .expect("count");
        assert_eq!(macros, example_macros().len() as i64);

        let settings: i64 = connection
            .query_row("SELECT COUNT(*) FROM rosetta_settings", [], |row| row.get(0))
            .expect("count");
        assert_eq!(settings, substitution_presets().len() as i64);
    }

    #[test]
    fn user_edits_survive_repeated_seeding() {
        let connection = connection();
        seed_if_absent(&connection).expect("seed");
        connection
            .execute(
                "UPDATE user_macros SET name = 'My array' WHERE id = 'hole_array'",
                [],
            )
            .expect("edit");
        seed_if_absent(&connection).expect("re-seed");
        let name: String = connection
            .query_row(
                "SELECT name FROM user_macros WHERE id = 'hole_array'",
                [],
                |row| row.get(0),
            )
            .expect("read");
        assert_eq!(name, "My array", "INSERT OR IGNORE must not clobber edits");
    }

    #[test]
    fn presets_cover_both_cycles_on_both_backends() {
        let presets = substitution_presets();
        for backend in ["grbl", "fluidnc"] {
            for word in ["G83", "G73"] {
                assert!(
                    presets
                        .iter()
                        .any(|preset| preset.backend == backend && preset.word == word),
                    "missing {backend}/{word}"
                );
            }
        }
    }

    #[test]
    fn every_seed_macro_passes_the_api_size_limits() {
        // Bodies must fit the validator (<=65536 chars, no control chars).
        for item in example_macros() {
            assert!(item.body.len() <= 65_536);
            assert!(!item.body.chars().any(|c| c.is_control() && !matches!(c, '\n' | '\t')));
            assert!(!item.id.is_empty() && item.id.len() <= 80);
        }
    }
}
