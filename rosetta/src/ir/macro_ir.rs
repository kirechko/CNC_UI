//! Macro IR — the single source of truth for authoring procedures.
//!
//! Implements contracts `CORE-M-01` (the data schema itself) and provides the
//! serde shape used by the declarative YAML frontend (`CORE-M-02`). The Macro
//! Engine (`rosetta::macro_engine`) treats [`MacroProgram`] as its only
//! representation (`MAC-C-01`): frontends produce it, emitters consume it, and
//! nothing else is ever stored.
//!
//! # Deviations from the ТЗ sketches
//!
//! * `triggers` lives on [`MacroProgram`] (not in the short §3.1 sketch) so
//!   that the YAML example of ТЗ №2 §4 round-trips through the IR unchanged.
//! * [`Trigger`] is an internally tagged enum (`type: button` …) instead of a
//!   flat struct with optional fields: every trigger kind gets exactly the
//!   fields it needs and typos are caught at parse time.
//! * `MacroId`/`TransformId` are plain `String` aliases. Newtypes would add
//!   friction for serde and cross-module calls without any type-safety win
//!   that matters in a single-crate system.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Identifier of a macro in the registry (e.g. `"tool_change"`).
pub type MacroId = String;
/// Identifier of a transform in the transform registry (e.g. `"Rotate"`).
pub type TransformId = String;

/// Execution class of a macro (ТЗ №2 §2 — the key classification).
///
/// * [`MacroClass::Generative`] — a G-code *generator*: expanded once,
///   offline, before streaming (`CORE-M-04`).
/// * [`MacroClass::Reactive`] — interacts with machine state *during*
///   execution, so it must be interpreted on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MacroClass {
    Generative,
    #[default]
    Reactive,
}

impl MacroClass {
    /// Snake-case YAML name of the class (`"generative"` / `"reactive"`),
    /// used when embedding a class into format strings of test fixtures.
    pub fn yaml_name(self) -> &'static str {
        match self {
            MacroClass::Generative => "generative",
            MacroClass::Reactive => "reactive",
        }
    }
}

/// The type annotation of a parameter or local variable (ТЗ №2 §5: the
/// language is static, so declarations carry explicit kinds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    #[default]
    Number,
    Text,
    Flag,
    /// List of values, used by `foreach` iteration sources.
    List,
}

/// A runtime value produced by expression evaluation. Lives in the IR module
/// because [`Expr::Num`]/[`Expr::Str`] deserialization defaults and the
/// interpreter share one vocabulary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Bool(bool),
    Num(f64),
    Str(String),
    Array(Vec<Value>),
    /// Absent value (uninitialized variable, `return` without expression).
    Null,
}

impl Default for Value {
    fn default() -> Self {
        Value::Num(0.0)
    }
}

impl Value {
    /// Coerce to a number for arithmetic. Non-numeric values evaluate to 0.0,
    /// matching the forgiving semantics of O-words (`#<unset> == 0`).
    pub fn as_num(&self) -> f64 {
        match self {
            Value::Num(n) => *n,
            Value::Bool(true) => 1.0,
            Value::Bool(false) => 0.0,
            Value::Str(text) => text.parse::<f64>().unwrap_or(0.0),
            Value::Array(_) | Value::Null => 0.0,
        }
    }

    /// Truthiness used by `if`/`while`/`Wait` conditions: zero, null and the
    /// empty string are false, everything else is true.
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Num(n) => *n != 0.0,
            Value::Str(text) => !text.is_empty(),
            Value::Array(items) => !items.is_empty(),
            Value::Null => false,
        }
    }

    /// Declared kind of this value, for variable-kind diagnostics.
    pub fn kind(&self) -> ValueKind {
        match self {
            Value::Num(_) => ValueKind::Number,
            Value::Str(_) => ValueKind::Text,
            Value::Bool(_) => ValueKind::Flag,
            Value::Array(_) => ValueKind::List,
            Value::Null => ValueKind::Number,
        }
    }
}

/// Descriptive header of a macro program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroMeta {
    /// Stable registry identifier (`tool_change`), used by `SubstituteMacro`.
    pub id: MacroId,
    /// Human-readable name shown in the UI panel.
    pub name: String,
    /// Optional icon hint for the UI tile (ТЗ №2 §7.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// Optional grouping category.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Execution class; defaults to `reactive` when omitted in YAML.
    #[serde(default)]
    pub class: MacroClass,
    /// Safety flag: UI must show a confirmation modal before running
    /// (ТЗ №2 §7.3, combined with per-trigger `require_confirm`).
    #[serde(default)]
    pub require_confirm: bool,
}

/// Declared input parameter of a macro or subroutine.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParamDecl {
    pub name: String,
    #[serde(default)]
    pub kind: ValueKind,
    /// Default expression evaluated when the caller omits the argument.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Expr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Declared local variable of a macro.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VarDecl {
    pub name: String,
    #[serde(default)]
    pub kind: ValueKind,
    /// Initial value; defaults to the zero-value of `kind` when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial: Option<Expr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Named subprogram callable via [`Stmt::Call`] inside the same macro.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Subroutine {
    pub name: String,
    #[serde(default)]
    pub params: Vec<ParamDecl>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub body: Vec<Stmt>,
}

/// Binary operator of [`Expr::Bin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// Unary operator of [`Expr::Unary`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnOp {
    Neg,
    Not,
}

/// Expression node of the Macro IR (`CORE-M-01`). Static and fully
/// serializable; the interpreter in `macro_engine` evaluates it against a
/// variable scope plus the [`Expr::Status`] accessor of the host runtime.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Expr {
    Num(f64),
    Str(String),
    Bool(bool),
    /// Read a variable, parameter, or subroutine return value by name.
    Var(String),
    /// Read a host status field (`"probe_trig"`, `"pos.z"`, `"state"`).
    Status(String),
    Array(Vec<Expr>),
    Index(Box<Expr>, Box<Expr>),
    Range {
        start: Box<Expr>,
        end: Box<Expr>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        step: Option<Box<Expr>>,
    },
    Bin {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Unary { op: UnOp, arg: Box<Expr> },
    /// Built-in pure function call (`min`, `max`, `abs`, `sqrt`).
    CallExpr { name: String, args: Vec<Expr> },
}

/// Statement node of the Macro IR (`CORE-M-01`). Every control-flow construct
/// is static data (block vectors), never offsets or labels, which keeps the
/// IR re-emittable to O-words or Jinja2 without re-parsing.
///
/// # Serialization shapes (documented deviation)
///
/// `Stmt`/[`Expr`] are externally tagged. JSON (the spec's second sanctioned
/// storage format) renders them as the familiar map shape
/// `{"gcode": {"template": "G0 X1"}}`. The `serde_yaml_ng` crate does not
/// accept that map shape and instead uses YAML tags, so hand-written YAML
/// looks like:
///
/// ```yaml
/// body:
///   - !gcode { template: "G0 X1" }
///   - !wait { cond: !status probe_trig }
/// ```
///
/// The two forms describe the identical IR; `YamlFrontend` accepts the tag
/// form and JSON frontends would accept the map form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stmt {
    /// G-code line with `{variable}` interpolation (ТЗ №2 §5).
    #[serde(rename = "gcode")]
    GCode { template: String },
    Delay { ms: Expr },
    /// Block until the host reports `state == "Idle"` (reactive only).
    /// Generative expansion renders it as an explicit `G4` dwell or errors.
    Wait { cond: Expr },
    SetVar { name: String, value: Expr },
    If {
        cond: Expr,
        then: Vec<Stmt>,
        #[serde(default)]
        otherwise: Vec<Stmt>,
    },
    While { cond: Expr, body: Vec<Stmt> },
    Repeat { count: Expr, body: Vec<Stmt> },
    #[serde(rename = "foreach")]
    ForEach { var: String, iter: Expr, body: Vec<Stmt> },
    /// Call a [`Subroutine`] by name.
    Call {
        name: String,
        #[serde(default)]
        args: Vec<Expr>,
    },
    /// Cross-subsystem call into Rosetta Transform (`MAC-T-03`).
    /// `id` is a transform kind name validated against `TRF-M-04`.
    InvokeTransform {
        id: TransformId,
        #[serde(default)]
        args: Vec<Expr>,
    },
    Return {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<Expr>,
    },
    Abort { reason: Expr },
    Comment { text: String },
}

/// Launch trigger of a macro (ТЗ №2 §4). Internally tagged by `type` exactly
/// as in the specification YAML example.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Trigger {
    /// UI tile press; may demand a confirmation modal.
    Button {
        #[serde(default)]
        require_confirm: bool,
    },
    /// Keyboard chord such as `"Ctrl+Shift+Z"`.
    Hotkey {
        keys: String,
        #[serde(default)]
        require_confirm: bool,
    },
    /// Intercept of an M-code while the program/macro streams (`M6`).
    MCode {
        code: u16,
        #[serde(default)]
        require_confirm: bool,
    },
    /// Machine event such as `on_homed` / `on_probe_done`.
    Event {
        name: String,
        #[serde(default)]
        require_confirm: bool,
    },
}

impl Trigger {
    /// Whether this trigger fires on a program M-code equal to `code`.
    pub fn matches_mcode(&self, code: u16) -> bool {
        matches!(self, Trigger::MCode { code: c, .. } if *c == code)
    }

    /// Whether this trigger fires on the machine event `name`
    /// (case-sensitive, spec uses `on_homed` style).
    pub fn matches_event(&self, name: &str) -> bool {
        matches!(self, Trigger::Event { name: n, .. } if n == name)
    }

    /// Safety: the engine must ask for confirmation before running through
    /// this trigger.
    pub fn requires_confirm(&self) -> bool {
        match self {
            Trigger::Button { require_confirm } => *require_confirm,
            Trigger::Hotkey { require_confirm, .. } => *require_confirm,
            Trigger::MCode { require_confirm, .. } => *require_confirm,
            Trigger::Event { require_confirm, .. } => *require_confirm,
        }
    }
}

/// A complete macro program — the root of the Macro IR (`CORE-M-01`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacroProgram {
    pub meta: MacroMeta,
    #[serde(default)]
    pub params: Vec<ParamDecl>,
    #[serde(default)]
    pub locals: Vec<VarDecl>,
    #[serde(default)]
    pub subs: Vec<Subroutine>,
    /// Launch triggers (deviation: kept on the root so YAML round-trips).
    #[serde(default)]
    pub triggers: Vec<Trigger>,
    pub body: Vec<Stmt>,
}

impl MacroProgram {
    /// Structural self-validation used by the YAML frontend right after
    /// deserialization. It checks the locally decidable rules only
    /// (empty id, duplicate names); cross-object reference checks live in
    /// `macro_engine::registry::MacroRegistry::validate_references`
    /// (`CORE-M-05`).
    pub fn validate(&self) -> Result<(), crate::error::RosettaError> {
        // Duplicate names across params/locals/subs are validation errors.
        let mut names = BTreeSet::new();
        let mut all: Vec<&str> = Vec::new();
        all.extend(self.params.iter().map(|p| p.name.as_str()));
        all.extend(self.locals.iter().map(|v| v.name.as_str()));
        all.extend(self.subs.iter().map(|s| s.name.as_str()));
        for name in &all {
            if !names.insert(*name) {
                return Err(crate::error::RosettaError::Validation {
                    reason: format!("duplicate name \"{name}\" in params/locals/subs"),
                });
            }
        }
        let mut sub_names = BTreeSet::new();
        for sub in &self.subs {
            if !sub_names.insert(sub.name.as_str()) {
                return Err(crate::error::RosettaError::Validation {
                    reason: format!("duplicate subroutine name \"{}\"", sub.name),
                });
            }
        }
        if self.meta.id.is_empty() {
            return Err(crate::error::RosettaError::Validation {
                reason: "macro id must not be empty".to_owned(),
            });
        }
        Ok(())
    }

    /// Convenience: look up a subroutine by name (`CORE-M-05` resolution).
    pub fn find_sub(&self, name: &str) -> Option<&Subroutine> {
        self.subs.iter().find(|s| s.name == name)
    }

    /// Collect the names of every `Stmt::Call` target in the program body
    /// and all subs — the core's static reference scan used by
    /// `validate_references` (`CORE-M-05`).
    pub fn called_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        for stmt in &self.body {
            collect_calls(stmt, &mut out);
        }
        for sub in &self.subs {
            for stmt in &sub.body {
                collect_calls(stmt, &mut out);
            }
        }
        out
    }
}

/// Internal: recursive `Stmt::Call` scanner used by [`MacroProgram::called_names`].
fn collect_calls(stmt: &Stmt, out: &mut Vec<String>) {
    match stmt {
        Stmt::Call { name, .. } => out.push(name.clone()),
        Stmt::If { then, otherwise, .. } => {
            for s in then.iter().chain(otherwise) {
                collect_calls(s, out);
            }
        }
        Stmt::While { body, .. } | Stmt::Repeat { body, .. } | Stmt::ForEach { body, .. } => {
            for s in body {
                collect_calls(s, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact YAML example of ТЗ №2 §4 must parse into the IR.
    #[test]
    fn spec_trigger_yaml_parses() {
        // serde_yaml_ng renders externally tagged Stmt/Expr as YAML tags
        // (see the serialization-shapes note on `Stmt`); triggers keep the
        // spec's `{ type: ... }` internal-tag map shape.
        let yaml = r#"
meta: { id: tool_change, name: "Tool change" }
body:
  - !gcode { template: "G91 G38.2 Z-50 F100" }
  - !wait { cond: !status probe_trig }
triggers:
  - { type: button, require_confirm: true }
  - { type: mcode, code: 6 }
  - { type: hotkey, keys: "Ctrl+T" }
"#;
        let program: MacroProgram = serde_yaml_ng::from_str(yaml).unwrap();
        assert_eq!(program.meta.id, "tool_change");
        assert_eq!(program.body.len(), 2);
        assert_eq!(program.triggers.len(), 3);
        assert!(program.triggers[1].matches_mcode(6));
        assert!(program.triggers[0].requires_confirm());
    }

    #[test]
    fn yaml_roundtrip_preserves_ir() {
        let program = MacroProgram {
            meta: MacroMeta {
                id: "drill_grid".into(),
                name: "Drill grid".into(),
                icon: None,
                category: Some("machining".into()),
                class: MacroClass::Generative,
                require_confirm: false,
            },
            params: vec![ParamDecl {
                name: "rows".into(),
                kind: ValueKind::Number,
                default: Some(Expr::Num(3.0)),
                description: None,
            }],
            locals: vec![],
            subs: vec![Subroutine {
                name: "peck".into(),
                params: vec![ParamDecl {
                    name: "depth".into(),
                    kind: ValueKind::Number,
                    default: None,
                    description: None,
                }],
                description: None,
                body: vec![Stmt::GCode { template: "G1 Z-{depth}".into() }],
            }],
            triggers: vec![Trigger::Button { require_confirm: true }],
            body: vec![Stmt::ForEach {
                var: "r".into(),
                iter: Expr::Range {
                    start: Box::new(Expr::Num(0.0)),
                    end: Box::new(Expr::Var("rows".into())),
                    step: None,
                },
                body: vec![Stmt::Call { name: "peck".into(), args: vec![Expr::Num(-5.0)] }],
            }],
        };
        let text = serde_yaml_ng::to_string(&program).unwrap();
        let back: MacroProgram = serde_yaml_ng::from_str(&text).unwrap();
        assert_eq!(program, back);
    }

    #[test]
    fn validate_rejects_duplicate_names() {
        let mut program = MacroProgram {
            meta: MacroMeta {
                id: "x".into(),
                name: "x".into(),
                icon: None,
                category: None,
                class: MacroClass::default(),
                require_confirm: false,
            },
            params: vec![
                ParamDecl { name: "a".into(), kind: ValueKind::Number, default: None, description: None },
                ParamDecl { name: "a".into(), kind: ValueKind::Number, default: None, description: None },
            ],
            locals: vec![],
            subs: vec![],
            triggers: vec![],
            body: vec![],
        };
        assert!(program.validate().is_err());
        program.params.pop();
        assert!(program.validate().is_ok());
    }

    #[test]
    fn calls_are_collected_recursively() {
        let stmts = vec![Stmt::If {
            cond: Expr::Bool(true),
            then: vec![Stmt::Call { name: "peck".into(), args: vec![] }],
            otherwise: vec![Stmt::While {
                cond: Expr::Bool(false),
                body: vec![Stmt::Call { name: "home".into(), args: vec![] }],
            }],
        }];
        let program = MacroProgram {
            meta: MacroMeta {
                id: "m".into(),
                name: "m".into(),
                icon: None,
                category: None,
                class: MacroClass::default(),
                require_confirm: false,
            },
            params: vec![],
            locals: vec![],
            subs: vec![],
            triggers: vec![],
            body: stmts,
        };
        let mut calls = program.called_names();
        calls.sort();
        assert_eq!(calls, vec!["home", "peck"]);
    }
}
