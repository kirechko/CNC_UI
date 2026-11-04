use super::expr::Expr;
use super::stmt::Stmt;
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
