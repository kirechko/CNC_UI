use serde::{Deserialize, Serialize};

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
    Unary {
        op: UnOp,
        arg: Box<Expr>,
    },
    /// Built-in pure function call (`min`, `max`, `abs`, `sqrt`).
    CallExpr {
        name: String,
        args: Vec<Expr>,
    },
}
