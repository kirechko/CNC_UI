//! The synchronous host interpreter (ТЗ №2 §6: state machine
//! `Idle → Running → (Paused) → Done/Aborted`, error handling, cancellation)
//! — the execution path for **reactive** macros (`MAC` §5 routing) and the
//! engine reused by the offline generative expander (`CORE-M-04`).
//!
//! One run of one program is owned by an [`Engine`] value: it holds mutable
//! borrows of the runtime, the cancellation token, the log sink and the
//! transform hook, which lets recursive statement execution thread all of
//! them without lifetime gymnastics at every call site.
//!
//! Guards (ТЗ №2 §6: static language, safe to run against a real machine):
//!
//! * [`MAX_DEPTH`] — subroutine call nesting limit.
//! * [`MAX_STEPS`] — total executed statements per run (kills infinite loops).
//! * [`MAX_WAIT_POLLS`] — bounded `Wait` polling (`Wait` must never spin forever).
//!
//! Convention (documented deviation): the value produced by a `Call` to a
//! subroutine, and by `InvokeTransform`, is additionally exposed to the
//! caller as the variable `"return"` — a host-side analog of O-words `#5062`.

use std::collections::HashMap;

use crate::error::{RosettaError, RosettaResult};
use crate::ir::macro_ir::{BinOp, Expr, MacroProgram, ParamDecl, Stmt, UnOp, Value, ValueKind};
use crate::macro_engine::runtime::{
    CancellationToken, HostRuntime, MachineState, MacroLogLine, MacroLogLevel, RunOutcome,
};

/// Maximum subroutine-call nesting depth.
pub const MAX_DEPTH: usize = 32;
/// Maximum total statements executed in a single macro run.
pub const MAX_STEPS: u64 = 200_000;
/// Maximum polling iterations of a single `Stmt::Wait`.
pub const MAX_WAIT_POLLS: u32 = 10_000;

/// The signature every `InvokeTransform` bridge must satisfy
/// (`MAC-T-03`): synchronous, returns `Ok(Some(value))` on success,
/// `Err` on failure (`MAC-T-05`).
pub type TransformHook<'h> =
    dyn FnMut(&str, Vec<Value>) -> RosettaResult<Option<Value>> + 'h;

/// Non-local control-flow signal that must unwind statement execution:
/// `Return`, `Abort` (`Stmt::Abort`), `Cancelled` (host stop / E-Stop), and
/// `Error` for ordinary failures that happen deep inside a block.
enum Signal {
    Return(Value),
    Abort(String),
    Cancelled,
    Error(RosettaError),
}

/// Internal result type for statement/block execution.
type Exec<T> = Result<T, Signal>;

/// The interpreter for one macro program on one host runtime.
///
/// `'a` ties together the mutable borrows (runtime, log, hook slot);
/// `'h` is the lifetime *inside* the transform hook slot — kept separate
/// because `&mut Option<…>` is invariant in its payload type.
pub struct Engine<'a, 'h> {
    /// The program being executed (borrowed out of the registry).
    program: &'a MacroProgram,
    /// Live machine seam.
    rt: &'a mut dyn HostRuntime,
    /// Cooperative cancellation (§6.4).
    cancel: &'a CancellationToken,
    /// `[MACRO]` log sink (§10).
    log: &'a mut dyn FnMut(MacroLogLine),
    /// Optional `InvokeTransform` bridge slot (`MAC-T-03/05/06`). Held as a
    /// reborrowable `&mut` inside the `Option` so callers can pass
    /// `&mut Option<&mut dyn ...>` through unchanged.
    transform: &'a mut Option<&'h mut TransformHook<'h>>,
    /// Scope stack: index 0 = macro globals (params + locals); subroutine
    /// calls push a new scope that sees enclosing scopes but not vice versa.
    scopes: Vec<HashMap<String, Value>>,
    /// Current subroutine nesting depth (guard against runaway recursion).
    depth: usize,
    /// Statements executed so far this run (guard).
    steps: u64,
    /// Value captured from the most recent `Return`/`InvokeTransform`.
    pending_return: Option<Value>,
}

impl<'a, 'h> Engine<'a, 'h> {
    /// Create an engine; does not execute anything yet.
    fn new(
        program: &'a MacroProgram,
        rt: &'a mut dyn HostRuntime,
        cancel: &'a CancellationToken,
        log: &'a mut dyn FnMut(MacroLogLine),
        transform: &'a mut Option<&'h mut TransformHook<'h>>,
    ) -> Self {
        Engine {
            program,
            rt,
            cancel,
            log,
            transform,
            scopes: vec![HashMap::new()],
            depth: 0,
            steps: 0,
            pending_return: None,
        }
    }

    /// Entry point: bind parameters, seed locals, run the body (§6.3:
    /// `Idle → Running → Done/Aborted`).
    fn run(
        program: &'a MacroProgram,
        args: Vec<Value>,
        rt: &'a mut dyn HostRuntime,
        cancel: &'a CancellationToken,
        log: &'a mut dyn FnMut(MacroLogLine),
        transform: &'a mut Option<&'h mut TransformHook<'h>>,
    ) -> RosettaResult<RunOutcome> {
        let mut engine = Engine::new(program, rt, cancel, log, transform);
        // Bind positional parameters to declared names; missing arguments
        // take the declared default or the typed zero value.
        for (index, param) in program.params.iter().enumerate() {
            let value = engine.bind_argument(args.get(index), param)?;
            engine.declare(&param.name, value);
        }
        for local in &program.locals {
            let value = match &local.initial {
                Some(expr) => engine.eval(expr)?,
                None => default_for(local.kind),
            };
            engine.declare(&local.name, value);
        }
        engine.log_line("start", MacroLogLevel::Info);
        let block = program.body.clone();
        match engine.exec_block(&block) {
            Ok(()) => Ok(RunOutcome::Completed { value: engine.pending_return }),
            Err(Signal::Return(value)) => Ok(RunOutcome::Completed { value: Some(value) }),
            Err(Signal::Abort(reason)) => {
                engine.log_line(&format!("abort: {reason}"), MacroLogLevel::Error);
                Ok(RunOutcome::Aborted { reason })
            }
            Err(Signal::Cancelled) => {
                engine.log_line("cancelled", MacroLogLevel::Warn);
                Ok(RunOutcome::Cancelled)
            }
            Err(Signal::Error(error)) => Err(error),
        }
    }

    /// Resolve one parameter binding: explicit arg → default expr → zero.
    fn bind_argument(
        &mut self,
        provided: Option<&Value>,
        param: &ParamDecl,
    ) -> RosettaResult<Value> {
        match provided {
            Some(value) => Ok(value.clone()),
            None => match &param.default {
                Some(default) => self.eval(default),
                None => Ok(default_for(param.kind)),
            },
        }
    }

    // ----- variable scope helpers -----------------------------------------

    /// Write a variable into the innermost scope declaring it, else the
    /// current scope — dynamic-scope semantics analogous to `#<name>`.
    fn set_var(&mut self, name: &str, value: Value) {
        for scope in self.scopes.iter_mut().rev() {
            if scope.contains_key(name) {
                scope.insert(name.to_owned(), value);
                return;
            }
        }
        self.scopes
            .last_mut()
            .expect("engine always keeps the global scope")
            .insert(name.to_owned(), value);
    }

    /// Declare a fresh variable in the current (top) scope.
    fn declare(&mut self, name: &str, value: Value) {
        self.scopes
            .last_mut()
            .expect("engine always keeps the global scope")
            .insert(name.to_owned(), value);
    }

    /// Read a variable, walking scopes innermost-out (`Expr::Var`).
    /// Unknown names are hard errors — silent zeros would distort motion.
    fn get_var(&self, name: &str) -> RosettaResult<Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.get(name) {
                return Ok(value.clone());
            }
        }
        Err(RosettaError::Validation {
            reason: format!("undefined variable \"{name}\""),
        })
    }

    // ----- logging ---------------------------------------------------------

    /// Emit one `[MACRO]` log line (ТЗ №2 §10).
    fn log_line(&mut self, message: &str, level: MacroLogLevel) {
        let line = MacroLogLine {
            macro_id: self.program.meta.id.clone(),
            step: Some((self.steps, None)),
            message: message.to_owned(),
            level,
        };
        (self.log)(line);
    }

    // ----- statement execution --------------------------------------------

    /// Execute a block sequentially; stops on the first error or signal.
    fn exec_block(&mut self, block: &[Stmt]) -> Exec<()> {
        for stmt in block {
            self.exec_stmt(stmt)?;
        }
        Ok(())
    }

    /// Execute one statement, honoring cancellation, step budget, and the
    /// alarm/E-Stop abort rules (§6.2, §6.4, §9.1).
    fn exec_stmt(&mut self, stmt: &Stmt) -> Exec<()> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            return Err(Signal::Error(RosettaError::Validation {
                reason: format!("macro exceeded the {MAX_STEPS}-step budget"),
            }));
        }
        if self.cancel.is_cancelled() {
            return Err(Signal::Cancelled);
        }
        match stmt {
            Stmt::Comment { .. } => Ok(()),
            Stmt::GCode { template } => {
                let command = self.interpolate(template).map_err(Signal::Error)?;
                self.log_line(&format!("send {command}"), MacroLogLevel::Info);
                let response = self.rt.send_command(&command).map_err(Signal::Error)?;
                // A command that drove the machine into alarm/E-Stop aborts
                // the run immediately (§6.2 "прерывается немедленно").
                let state = self.rt.machine_state();
                if state.must_abort() {
                    self.log_line(
                        &format!("machine entered {state:?} during {command}"),
                        MacroLogLevel::Error,
                    );
                    return Err(if state == MachineState::EStop {
                        Signal::Cancelled
                    } else {
                        Signal::Error(RosettaError::Runtime {
                            reason: format!("controller alarm after {command}: {response}"),
                        })
                    });
                }
                Ok(())
            }
            Stmt::Delay { ms } => {
                let ms = self.eval(ms).map_err(Signal::Error)?.as_num().max(0.0) as u64;
                self.log_line(&format!("delay {ms}ms"), MacroLogLevel::Info);
                self.rt.sleep_ms(ms);
                Ok(())
            }
            Stmt::Wait { cond } => self.exec_wait(cond),
            Stmt::SetVar { name, value } => {
                let value = self.eval(value).map_err(Signal::Error)?;
                self.set_var(name, value);
                Ok(())
            }
            Stmt::If { cond, then, otherwise } => {
                let taken = self.eval(cond).map_err(Signal::Error)?.is_truthy();
                self.log_line(&format!("condition -> {taken}"), MacroLogLevel::Info);
                self.exec_block(if taken { then } else { otherwise })
            }
            Stmt::While { cond, body } => loop {
                if self.cancel.is_cancelled() {
                    return Err(Signal::Cancelled);
                }
                if !self.eval(cond).map_err(Signal::Error)?.is_truthy() {
                    return Ok(());
                }
                self.exec_block(body)?;
            },
            Stmt::Repeat { count, body } => {
                let count = self.eval(count).map_err(Signal::Error)?.as_num();
                if !count.is_finite() || count < 0.0 || count.fract() != 0.0 {
                    return Err(Signal::Error(RosettaError::Validation {
                        reason: format!("repeat count must be a non-negative integer, got {count}"),
                    }));
                }
                for _ in 0..count as u64 {
                    if self.cancel.is_cancelled() {
                        return Err(Signal::Cancelled);
                    }
                    self.exec_block(body)?;
                }
                Ok(())
            }
            Stmt::ForEach { var, iter, body } => {
                let items = self.expand_iter(iter).map_err(Signal::Error)?;
                for item in items {
                    if self.cancel.is_cancelled() {
                        return Err(Signal::Cancelled);
                    }
                    self.set_var(var, item);
                    self.exec_block(body)?;
                }
                Ok(())
            }
            Stmt::Call { name, args } => self.exec_call(name, args),
            Stmt::InvokeTransform { id, args } => {
                let values = self.eval_args(args).map_err(Signal::Error)?;
                self.log_line(&format!("transform {id}"), MacroLogLevel::Info);
                // MAC-T-03/05: synchronous call into the transform engine.
                let result = match &mut *self.transform {
                    Some(hook) => hook(id.as_str(), values).map_err(Signal::Error)?,
                    None => {
                        return Err(Signal::Error(RosettaError::NotFound {
                            kind: "transform bridge".into(),
                            id: id.clone(),
                        }))
                    }
                };
                // MAC-T-06: cancellation raised during the transform stops
                // the macro before any further motion.
                if self.cancel.is_cancelled() {
                    return Err(Signal::Cancelled);
                }
                if let Some(value) = result {
                    self.pending_return = Some(value.clone());
                    self.set_var("return", value);
                }
                Ok(())
            }
            Stmt::Return { value } => {
                let value = match value {
                    Some(expr) => self.eval(expr).map_err(Signal::Error)?,
                    None => Value::Null,
                };
                Err(Signal::Return(value))
            }
            Stmt::Abort { reason } => {
                let reason = self.eval(reason).map_err(Signal::Error)?.to_display_string();
                Err(Signal::Abort(reason))
            }
        }
    }

    /// `Stmt::Wait`: poll the condition until truthy, bounded, with the same
    /// cancel/alarm rules as any loop (ТЗ №2 §6.5 reactive path).
    fn exec_wait(&mut self, cond: &Expr) -> Exec<()> {
        for poll in 0..MAX_WAIT_POLLS {
            if self.cancel.is_cancelled() {
                return Err(Signal::Cancelled);
            }
            if self.eval(cond).map_err(Signal::Error)?.is_truthy() {
                return Ok(());
            }
            let state = self.rt.machine_state();
            if state.must_abort() {
                return Err(Signal::Error(RosettaError::Runtime {
                    reason: format!("machine in {state:?} while waiting"),
                }));
            }
            if poll == 0 {
                self.log_line("wait: polling status…", MacroLogLevel::Info);
            }
            self.rt.sleep_ms(50);
        }
        Err(Signal::Error(RosettaError::Runtime {
            reason: format!("wait condition did not become true within {MAX_WAIT_POLLS} polls"),
        }))
    }

    /// `Stmt::Call`: push a scope, bind parameters, execute the sub body,
    /// pop. A `Return` from the sub is consumed here (captured as `"return"`)
    /// so only a *top-level* Return can end the whole macro.
    fn exec_call(&mut self, name: &str, args: &[Expr]) -> Exec<()> {
        // Copy the sub body out of `self.program` so execution can mutate
        // scopes while holding no borrow of the program itself.
        let body = self
            .program
            .find_sub(name)
            .ok_or_else(|| Signal::Error(RosettaError::NotFound {
                kind: "subroutine".into(),
                id: name.to_owned(),
            }))?
            .clone();
        if self.depth + 1 > MAX_DEPTH {
            return Err(Signal::Error(RosettaError::Validation {
                reason: format!("subroutine nesting exceeds MAX_DEPTH ({MAX_DEPTH})"),
            }));
        }
        let values = self.eval_args(args).map_err(Signal::Error)?;
        self.scopes.push(HashMap::new());
        self.depth += 1;
        for (index, param) in body.params.iter().enumerate() {
            let value = match values.get(index) {
                Some(value) => value.clone(),
                None => match &param.default {
                    Some(default) => self.eval(default).map_err(Signal::Error)?,
                    None => default_for(param.kind),
                },
            };
            self.declare(&param.name, value);
        }
        let stmts = body.body.clone();
        let result = self.exec_block(&stmts);
        self.depth -= 1;
        self.scopes.pop();
        match result {
            Ok(()) => Ok(()),
            Err(Signal::Return(value)) => {
                self.pending_return = Some(value.clone());
                self.set_var("return", value);
                Ok(())
            }
            Err(signal) => Err(signal),
        }
    }

    // ----- expression evaluation -------------------------------------------

    /// Evaluate an expression to a runtime value.
    fn eval(&mut self, expr: &Expr) -> RosettaResult<Value> {
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
    fn eval_args(&mut self, args: &[Expr]) -> RosettaResult<Vec<Value>> {
        let mut out = Vec::with_capacity(args.len());
        for arg in args {
            out.push(self.eval(arg)?);
        }
        Ok(out)
    }

    /// Materialize a `start..end[:step]` range into numbers, capped by the
    /// expansion budget.
    fn range_items(
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
        Ok((0..count).map(|i| Value::Num(start + i as f64 * step)).collect())
    }

    /// Resolve the iteration source of `Stmt::ForEach`: arrays and ranges
    /// expand; any other single value iterates once.
    fn expand_iter(&mut self, iter: &Expr) -> RosettaResult<Vec<Value>> {
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
    fn interpolate(&mut self, template: &str) -> RosettaResult<String> {
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

/// Public entry point: run `program` against `rt`.
///
/// * Reactive path — `rt` is a live runtime (ТЗ №2 §6.5 host interpretation).
/// * Generative path — `rt` is an [`crate::macro_engine::runtime::EmitRuntime`]
///   collector and the run performs offline expansion instead of motion.
/// * `transform` — optional `InvokeTransform` bridge slot (`MAC-T-03`).
///
/// Transport/controller failures surface as `Err(RosettaError)`; semantic
/// outcomes (`Abort`, cancellation) surface as `Ok(RunOutcome)`.
pub fn run_macro<'h>(
    program: &MacroProgram,
    args: Vec<Value>,
    rt: &mut dyn HostRuntime,
    cancel: &CancellationToken,
    log: &mut dyn FnMut(MacroLogLine),
    transform: &mut Option<&'h mut TransformHook<'h>>,
) -> RosettaResult<RunOutcome> {
    Engine::run(program, args, rt, cancel, log, transform)
}

/// Zero/default value of a declared kind (params/locals without defaults).
fn default_for(kind: ValueKind) -> Value {
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
                    return Err(RosettaError::Runtime { reason: "division by zero".into() })
                }
                Div => a / b,
                Mod if b == 0.0 => {
                    return Err(RosettaError::Runtime { reason: "modulo by zero".into() })
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

/// Rendering of values into G-code templates and abort reasons: numbers use
/// the shared deterministic formatter, booleans become address-safe `1`/`0`.
pub(crate) trait ToDisplayString {
    fn to_display_string(&self) -> String;
}

impl ToDisplayString for Value {
    fn to_display_string(&self) -> String {
        match self {
            Value::Num(n) => crate::util::fmt_num(*n),
            Value::Str(text) => text.clone(),
            Value::Bool(true) => "1".into(),
            Value::Bool(false) => "0".into(),
            Value::Null => String::new(),
            Value::Array(items) => items
                .iter()
                .map(ToDisplayString::to_display_string)
                .collect::<Vec<_>>()
                .join(","),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::macro_ir::{MacroClass, MacroMeta};
    use crate::macro_engine::runtime::ScriptedRuntime;

    /// Build a single-subject test program without YAML noise.
    fn program(body: Vec<Stmt>) -> MacroProgram {
        MacroProgram {
            meta: MacroMeta {
                id: "m".into(),
                name: "M".into(),
                icon: None,
                category: None,
                class: MacroClass::Reactive,
                require_confirm: false,
            },
            params: vec![],
            locals: vec![],
            subs: vec![],
            triggers: vec![],
            body,
        }
    }

    /// Helper: run an IR program against a canned status map; the second
    /// return value is the runtime with the in-memory command log.
    fn run(
        program: &MacroProgram,
        args: Vec<Value>,
        status: HashMap<String, Value>,
        transform: Option<&mut dyn FnMut(&str, Vec<Value>) -> RosettaResult<Option<Value>>>,
    ) -> (RosettaResult<RunOutcome>, ScriptedRuntime) {
        let mut rt = ScriptedRuntime::new(status, MachineState::Idle);
        let cancel = CancellationToken::new();
        let mut log = |_line: MacroLogLine| {};
        let mut holder = transform;
        let result = run_macro(program, args, &mut rt, &cancel, &mut log, &mut holder);
        (result, rt)
    }

    fn bin(op: BinOp, a: Expr, b: Expr) -> Expr {
        Expr::Bin { op, lhs: Box::new(a), rhs: Box::new(b) }
    }

    #[test]
    fn gcode_interpolates_numbers_without_trailing_zeros() {
        let mut p = program(vec![Stmt::GCode {
            template: "G0 X{x} Y{y}".into(),
        }]);
        p.params = vec![ParamDecl {
            name: "x".into(),
            kind: ValueKind::Number,
            default: None,
            description: None,
        }];
        p.locals = vec![crate::ir::macro_ir::VarDecl {
            name: "y".into(),
            kind: ValueKind::Number,
            initial: Some(Expr::Num(5.0)),
            description: None,
        }];
        let (outcome, rt) = run(&p, vec![Value::Num(10.25)], HashMap::new(), None);
        assert!(matches!(outcome.unwrap(), RunOutcome::Completed { .. }));
        assert_eq!(rt.sent, vec!["G0 X10.25 Y5"]);
    }

    #[test]
    fn reactive_if_else_selects_branch() {
        let p = program(vec![Stmt::If {
            cond: bin(BinOp::Gt, Expr::Num(3.0), Expr::Num(1.0)),
            then: vec![Stmt::GCode { template: "M100".into() }],
            otherwise: vec![Stmt::GCode { template: "M101".into() }],
        }]);
        let (outcome, rt) = run(&p, vec![], HashMap::new(), None);
        assert!(outcome.is_ok());
        assert_eq!(rt.sent, vec!["M100"]);
    }

    #[test]
    fn while_loop_consumes_status_and_completes() {
        // probe_trig starts at 0 => loop never runs; then G92 is sent.
        let mut status = HashMap::new();
        status.insert("probe_trig".to_string(), Value::Num(0.0));
        let p = program(vec![
            Stmt::While {
                cond: bin(BinOp::Gt, Expr::Status("probe_trig".into()), Expr::Num(0.0)),
                body: vec![Stmt::GCode { template: "G4 P0".into() }],
            },
            Stmt::GCode { template: "G92 Z0".into() },
        ]);
        let (outcome, rt) = run(&p, vec![], status, None);
        assert!(matches!(outcome.unwrap(), RunOutcome::Completed { value: None }));
        assert_eq!(rt.sent, vec!["G92 Z0"]);
    }

    #[test]
    fn wait_returns_when_condition_holds() {
        let mut status = HashMap::new();
        status.insert("idle".to_string(), Value::Bool(true));
        let p = program(vec![
            Stmt::Wait { cond: Expr::Status("idle".into()) },
            Stmt::GCode { template: "G0 X0".into() },
        ]);
        let (outcome, rt) = run(&p, vec![], status, None);
        assert!(outcome.is_ok());
        assert_eq!(rt.sent, vec!["G0 X0"]);
    }

    #[test]
    fn wait_surfaces_runtime_error_when_machine_alarms() {
        let mut status = HashMap::new();
        status.insert("idle".to_string(), Value::Bool(false));
        let mut rt = ScriptedRuntime::new(status, MachineState::Alarm);
        let cancel = CancellationToken::new();
        let mut log = |_line: MacroLogLine| {};
        let mut holder: Option<&mut TransformHook<'_>> = None;
        let p = program(vec![Stmt::Wait { cond: Expr::Status("idle".into()) }]);
        let error = run_macro(&p, vec![], &mut rt, &cancel, &mut log, &mut holder).unwrap_err();
        assert!(matches!(error, RosettaError::Runtime { .. }));
    }

    #[test]
    fn abort_yields_aborted_outcome_not_error() {
        let p = program(vec![
            Stmt::Abort { reason: Expr::Str("no tool".into()) },
            Stmt::GCode { template: "G1 X1".into() },
        ]);
        let (outcome, rt) = run(&p, vec![], HashMap::new(), None);
        match outcome.unwrap() {
            RunOutcome::Aborted { reason } => assert_eq!(reason, "no tool"),
            other => panic!("unexpected {other:?}"),
        }
        assert!(rt.sent.is_empty());
    }

    #[test]
    fn cancellation_stops_run() {
        let p = program(vec![Stmt::Repeat {
            count: Expr::Num(1000.0),
            body: vec![Stmt::GCode { template: "G4 P1".into() }],
        }]);
        let mut rt = ScriptedRuntime::default();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let mut log = |_line: MacroLogLine| {};
        let mut holder: Option<&mut TransformHook<'_>> = None;
        let outcome = run_macro(&p, vec![], &mut rt, &cancel, &mut log, &mut holder).unwrap();
        assert_eq!(outcome, RunOutcome::Cancelled);
        assert!(rt.sent.is_empty());
    }

    #[test]
    fn subroutine_return_captures_value_into_return_var() {
        use crate::ir::macro_ir::Subroutine;
        let mut p = program(vec![
            Stmt::Call { name: "double".into(), args: vec![Expr::Num(21.0)] },
            Stmt::SetVar {
                name: "result".into(),
                value: Expr::Var("return".into()),
            },
            Stmt::If {
                cond: bin(BinOp::Eq, Expr::Var("result".into()), Expr::Num(42.0)),
                then: vec![Stmt::GCode { template: "M999".into() }],
                otherwise: vec![],
            },
        ]);
        p.subs = vec![Subroutine {
            name: "double".into(),
            params: vec![ParamDecl {
                name: "v".into(),
                kind: ValueKind::Number,
                default: None,
                description: None,
            }],
            description: None,
            body: vec![Stmt::Return { value: Some(bin(BinOp::Mul, Expr::Var("v".into()), Expr::Num(2.0))) }],
        }];
        let (outcome, rt) = run(&p, vec![], HashMap::new(), None);
        assert!(outcome.is_ok());
        assert_eq!(rt.sent, vec!["M999"]);
    }

    #[test]
    fn invoke_transform_uses_bridge_and_exposes_return() {
        let p = program(vec![
            Stmt::InvokeTransform { id: "Rotate".into(), args: vec![Expr::Num(90.0)] },
            Stmt::GCode { template: "M100 {return}".into() },
        ]);
        let mut calls: Vec<(String, Vec<Value>)> = Vec::new();
        let mut hook = |id: &str, args: Vec<Value>| -> RosettaResult<Option<Value>> {
            calls.push((id.to_owned(), args));
            Ok(Some(Value::Num(1.0)))
        };
        let (outcome, rt) = run(&p, vec![], HashMap::new(), Some(&mut hook));
        assert!(outcome.is_ok());
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, "Rotate");
        assert_eq!(calls[0].1, vec![Value::Num(90.0)]);
        assert_eq!(rt.sent, vec!["M100 1"]);
    }

    #[test]
    fn invoke_transform_without_bridge_fails() {
        let p = program(vec![Stmt::InvokeTransform {
            id: "Rotate".into(),
            args: vec![],
        }]);
        let (outcome, _rt) = run(&p, vec![], HashMap::new(), None);
        assert!(matches!(
            outcome.unwrap_err(),
            RosettaError::NotFound { ref kind, .. } if kind == "transform bridge"
        ));
    }

    #[test]
    fn step_budget_kills_infinite_loop() {
        let p = program(vec![Stmt::While {
            cond: Expr::Bool(true),
            body: vec![Stmt::Comment { text: "tick".into() }],
        }]);
        let (outcome, _rt) = run(&p, vec![], HashMap::new(), None);
        match outcome {
            Err(RosettaError::Validation { reason }) => assert!(reason.contains("step budget")),
            other => panic!("expected step budget error, got {other:?}"),
        }
    }

    #[test]
    fn unknown_variable_is_hard_error() {
        let p = program(vec![Stmt::SetVar {
            name: "a".into(),
            value: Expr::Var("does_not_exist".into()),
        }]);
        let (outcome, _rt) = run(&p, vec![], HashMap::new(), None);
        assert!(matches!(
            outcome.unwrap_err(),
            RosettaError::Validation { ref reason } if reason.contains("does_not_exist")
        ));
    }

    #[test]
    fn repeat_rejects_non_integer_count() {
        let p = program(vec![Stmt::Repeat {
            count: Expr::Num(2.5),
            body: vec![],
        }]);
        let (outcome, _rt) = run(&p, vec![], HashMap::new(), None);
        assert!(matches!(outcome.unwrap_err(), RosettaError::Validation { .. }));
    }

    #[test]
    fn foreach_over_range_visits_each_item() {
        let p = program(vec![Stmt::ForEach {
            var: "c".into(),
            iter: Expr::Range {
                start: Box::new(Expr::Num(0.0)),
                end: Box::new(Expr::Num(3.0)),
                step: None,
            },
            body: vec![Stmt::GCode { template: "G1 X{c}".into() }],
        }]);
        let (outcome, rt) = run(&p, vec![], HashMap::new(), None);
        assert!(outcome.is_ok());
        assert_eq!(rt.sent, vec!["G1 X0", "G1 X1", "G1 X2"]);
    }

    #[test]
    fn builtins_evaluate_expected_values() {
        let mut e = Expr::CallExpr {
            name: "min".into(),
            args: vec![Expr::Num(7.0), Expr::Num(2.0), Expr::Num(5.0)],
        };
        // Bind every engine input to a local: `Engine` holds borrows that
        // outlive this expression, so temporaries would be dropped too early.
        let p = program(vec![]);
        let mut rt = ScriptedRuntime::default();
        let cancel = CancellationToken::new();
        let mut log = |_l: MacroLogLine| {};
        let mut holder: Option<&mut TransformHook<'_>> = None;
        let mut scope = Engine::new(&p, &mut rt, &cancel, &mut log, &mut holder);
        assert_eq!(scope.eval(&e).unwrap(), Value::Num(2.0));
        e = Expr::Unary { op: UnOp::Not, arg: Box::new(Expr::Bool(false)) };
        assert_eq!(scope.eval(&e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn interpolation_requires_balance_and_escapes() {
        // An unterminated placeholder must be a hard parse error...
        let p = program(vec![Stmt::GCode { template: "G4 {x".into() }]);
        // ...while a doubled brace is a literal escape and `{` inside plain
        // text that is not a `{name}` placeholder stays untouched only when
        // it is escaped; here the stray `{x` after the escape is unterminated
        // too, so this program must also fail.
        let q = program(vec![Stmt::GCode { template: "G4 {{literal {x".into() }]);
        let mut r = program(vec![Stmt::GCode { template: "G4 {x}".into() }]);
        r.params = vec![ParamDecl {
            name: "x".into(),
            kind: ValueKind::Number,
            default: Some(Expr::Num(1.0)),
            description: None,
        }];
        let (outcome, _rt) = run(&p, vec![], HashMap::new(), None);
        assert!(outcome.is_err(), "unterminated placeholder must fail");
        let (outcome, _rt) = run(&q, vec![], HashMap::new(), None);
        assert!(outcome.is_err(), "escape must not hide an unterminated brace");
        let (outcome, rt) = run(&r, vec![], HashMap::new(), None);
        assert!(outcome.is_ok());
        assert_eq!(rt.sent, vec!["G4 1"]);
    }
}
