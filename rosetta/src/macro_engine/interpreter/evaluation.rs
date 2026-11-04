//! Expression evaluation, iteration expansion, and template interpolation.

use crate::error::{RosettaError, RosettaResult};
use crate::ir::macro_ir::{BinOp, Expr, UnOp, Value, ValueKind};

use super::ToDisplayString;
use super::{Engine, MAX_STEPS};

impl<'a, 'h> Engine<'a, 'h> {
    // ----- expression evaluation -------------------------------------------

    /// Evaluate an expression to a runtime value.
    pub(super) fn eval(&mut self, expr: &Expr) -> RosettaResult<Value> {
        match expr {
            Expr::Num(n) => Ok(Value::Num(*n)),
            Expr::Str(text) => Ok(Value::Str(text.clone())),
            Expr::Bool(b) => Ok(Value::Bool(*b)),
            Expr::Var(name) => self.get_var(name),
            Expr::Status(key) => self.rt.status_field(key),
            Expr::Array(items) => Ok(Value::Array(self.eval_args(items)?)),
            Expr::Index(base, index) => {
                let base = self.eval(base)?;
                let index = self.eval(index)?.as_num().floor();
                match base {
                    Value::Array(items) if index >= 0.0 && (index as usize) < items.len() => {
                        Ok(items[index as usize].clone())
                    }
                    Value::Array(_) => Err(RosettaError::Validation {
                        reason: format!("array index {index} out of bounds"),
                    }),
                    other => Err(RosettaError::Validation {
                        reason: format!("cannot index a {:?}", other.kind()),
                    }),
                }
            }
            Expr::Range { start, end, step } => {
                let items = self.range_items(start, end, step.as_deref())?;
                Ok(Value::Array(items))
            }
            Expr::Bin { op, lhs, rhs } => {
                let lhs = self.eval(lhs)?;
                let rhs = self.eval(rhs)?;
                eval_bin(*op, lhs, rhs)
            }
            Expr::Unary { op, arg } => {
                let value = self.eval(arg)?;
                Ok(eval_unary(*op, value))
            }
            Expr::CallExpr { name, args } => {
                let values = self.eval_args(args)?;
                eval_builtin(name, values)
            }
        }
    }

    /// Evaluate a positional argument list.
    pub(super) fn eval_args(&mut self, args: &[Expr]) -> RosettaResult<Vec<Value>> {
        let mut out = Vec::with_capacity(args.len());
        for arg in args {
            out.push(self.eval(arg)?);
        }
        Ok(out)
    }

    /// Materialize a `start..end[:step]` range into numbers, capped by the
    /// expansion budget.
    pub(super) fn range_items(
        &mut self,
        start: &Expr,
        end: &Expr,
        step: Option<&Expr>,
    ) -> RosettaResult<Vec<Value>> {
        let start = self.eval(start)?.as_num();
        let end = self.eval(end)?.as_num();
        let step = match step {
            Some(expr) => self.eval(expr)?.as_num(),
            None => 1.0,
        };
        if step == 0.0 || !step.is_finite() {
            return Err(RosettaError::Validation {
                reason: "range step must be a non-zero finite number".into(),
            });
        }
        let span = if step > 0.0 { end - start } else { start - end };
        let count = (span / step.abs()).max(0.0).ceil() as usize;
        if count > MAX_STEPS as usize {
            return Err(RosettaError::Validation {
                reason: format!("range of {count} items exceeds the expansion budget"),
            });
        }
        Ok((0..count)
            .map(|i| Value::Num(start + i as f64 * step))
            .collect())
    }

    /// Resolve the iteration source of `Stmt::ForEach`: arrays and ranges
    /// expand; any other single value iterates once.
    pub(super) fn expand_iter(&mut self, iter: &Expr) -> RosettaResult<Vec<Value>> {
        match iter {
            Expr::Array(items) => self.eval_args(items),
            Expr::Range { start, end, step } => self.range_items(start, end, step.as_deref()),
            other => Ok(vec![self.eval(other)?]),
        }
    }

    // ----- template interpolation ------------------------------------------

    /// Expand a `Stmt::GCode` template: `{name}` inserts the variable value
    /// (numbers via the shared deterministic formatter), `{{`/`}}` are
    /// literal braces, unknown names are hard errors (ТЗ №1 principle 4).
    pub(super) fn interpolate(&mut self, template: &str) -> RosettaResult<String> {
        let mut out = String::with_capacity(template.len());
        let chars: Vec<char> = template.chars().collect();
        let mut index = 0usize;
        while index < chars.len() {
            let ch = chars[index];
            if ch == '{' {
                if index + 1 < chars.len() && chars[index + 1] == '{' {
                    out.push('{');
                    index += 2;
                    continue;
                }
                let close = chars[index..]
                    .iter()
                    .position(|c| *c == '}')
                    .ok_or_else(|| RosettaError::Parse {
                        line: 0,
                        reason: format!("unterminated placeholder in template \"{template}\""),
                    })?;
                let name: String = chars[index + 1..index + close].iter().collect();
                let value = self.get_var(&name)?;
                out.push_str(&value.to_display_string());
                index += close + 1;
                continue;
            }
            if ch == '}' && chars.get(index + 1) == Some(&'}') {
                out.push('}');
                index += 2;
                continue;
            }
            out.push(ch);
            index += 1;
        }
        Ok(out)
    }
}

/// Zero/default value of a declared kind (params/locals without defaults).
pub(super) fn default_for(kind: ValueKind) -> Value {
    match kind {
        ValueKind::Number => Value::Num(0.0),
        ValueKind::Text => Value::Str(String::new()),
        ValueKind::Flag => Value::Bool(false),
        ValueKind::List => Value::Array(Vec::new()),
    }
}

/// Evaluate a binary operation with forgiving coercions (`MAC` §5: numeric,
/// text and boolean values support assignment and comparison; text compares
/// lexicographically, everything else numerically).
fn eval_bin(op: BinOp, lhs: Value, rhs: Value) -> RosettaResult<Value> {
    use BinOp::*;
    Ok(match op {
        Add | Sub | Mul | Div | Mod => {
            let (a, b) = (lhs.as_num(), rhs.as_num());
            let n = match op {
                Add => a + b,
                Sub => a - b,
                Mul => a * b,
                Div if b == 0.0 => {
                    return Err(RosettaError::Runtime {
                        reason: "division by zero".into(),
                    })
                }
                Div => a / b,
                Mod if b == 0.0 => {
                    return Err(RosettaError::Runtime {
                        reason: "modulo by zero".into(),
                    })
                }
                Mod => a % b,
                _ => unreachable!("comparison ops handled below"),
            };
            Value::Num(n)
        }
        Eq | Ne | Lt | Le | Gt | Ge => {
            let ordering = match (&lhs, &rhs) {
                (Value::Str(a), Value::Str(b)) => a.as_str().cmp(b.as_str()),
                _ => lhs
                    .as_num()
                    .partial_cmp(&rhs.as_num())
                    .unwrap_or(std::cmp::Ordering::Less),
            };
            let ok = match op {
                Eq => ordering == std::cmp::Ordering::Equal,
                Ne => ordering != std::cmp::Ordering::Equal,
                Lt => ordering == std::cmp::Ordering::Less,
                Le => ordering != std::cmp::Ordering::Greater,
                Gt => ordering == std::cmp::Ordering::Greater,
                Ge => ordering != std::cmp::Ordering::Less,
                _ => unreachable!("arithmetic ops handled above"),
            };
            Value::Bool(ok)
        }
        And => Value::Bool(lhs.is_truthy() && rhs.is_truthy()),
        Or => Value::Bool(lhs.is_truthy() || rhs.is_truthy()),
    })
}

/// Evaluate a unary operation.
fn eval_unary(op: UnOp, value: Value) -> Value {
    match op {
        UnOp::Neg => Value::Num(-value.as_num()),
        UnOp::Not => Value::Bool(!value.is_truthy()),
    }
}

/// Built-in pure functions available to `Expr::CallExpr`. The set is
/// deliberately tiny and statically analyzable (ТЗ №2 §5: static language).
fn eval_builtin(name: &str, args: Vec<Value>) -> RosettaResult<Value> {
    let mut nums = args.iter().map(Value::as_num);
    let mut one = |name: &str| -> RosettaResult<f64> {
        nums.next().ok_or_else(|| RosettaError::Validation {
            reason: format!("function {name} called without arguments"),
        })
    };
    match name {
        // Variadic folds:
        "min" => {
            let first = one("min")?;
            Ok(Value::Num(nums.fold(first, f64::min)))
        }
        "max" => {
            let first = one("max")?;
            Ok(Value::Num(nums.fold(first, f64::max)))
        }
        // Single-argument:
        "abs" => Ok(Value::Num(one("abs")?.abs())),
        "sqrt" => Ok(Value::Num(one("sqrt")?.max(0.0).sqrt())),
        "floor" => Ok(Value::Num(one("floor")?.floor())),
        "ceil" => Ok(Value::Num(one("ceil")?.ceil())),
        "round" => Ok(Value::Num(one("round")?.round())),
        _ => Err(RosettaError::NotFound {
            kind: "builtin function".into(),
            id: name.to_owned(),
        }),
    }
}
