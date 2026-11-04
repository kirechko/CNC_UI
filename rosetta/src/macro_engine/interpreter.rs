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
use crate::ir::macro_ir::{MacroProgram, Value};
use crate::macro_engine::runtime::{CancellationToken, HostRuntime, MacroLogLine, RunOutcome};

/// Maximum subroutine-call nesting depth.
pub const MAX_DEPTH: usize = 32;
/// Maximum total statements executed in a single macro run.
pub const MAX_STEPS: u64 = 200_000;
/// Maximum polling iterations of a single `Stmt::Wait`.
pub const MAX_WAIT_POLLS: u32 = 10_000;

/// The signature every `InvokeTransform` bridge must satisfy
/// (`MAC-T-03`): synchronous, returns `Ok(Some(value))` on success,
/// `Err` on failure (`MAC-T-05`).
pub type TransformHook<'h> = dyn FnMut(&str, Vec<Value>) -> RosettaResult<Option<Value>> + 'h;

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

mod evaluation;
mod execution;

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

/// Rendering of values into G-code templates and abort reasons: numbers use
/// the shared deterministic formatter, booleans become address-safe `1`/`0`.
pub(crate) trait ToDisplayString {
    /// Render this value in the interpreter's deterministic display format.
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
    use crate::ir::macro_ir::{
        BinOp, Expr, MacroClass, MacroMeta, ParamDecl, Stmt, UnOp, ValueKind,
    };
    use crate::macro_engine::runtime::{MachineState, ScriptedRuntime};

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
        Expr::Bin {
            op,
            lhs: Box::new(a),
            rhs: Box::new(b),
        }
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
            then: vec![Stmt::GCode {
                template: "M100".into(),
            }],
            otherwise: vec![Stmt::GCode {
                template: "M101".into(),
            }],
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
                body: vec![Stmt::GCode {
                    template: "G4 P0".into(),
                }],
            },
            Stmt::GCode {
                template: "G92 Z0".into(),
            },
        ]);
        let (outcome, rt) = run(&p, vec![], status, None);
        assert!(matches!(
            outcome.unwrap(),
            RunOutcome::Completed { value: None }
        ));
        assert_eq!(rt.sent, vec!["G92 Z0"]);
    }

    #[test]
    fn wait_returns_when_condition_holds() {
        let mut status = HashMap::new();
        status.insert("idle".to_string(), Value::Bool(true));
        let p = program(vec![
            Stmt::Wait {
                cond: Expr::Status("idle".into()),
            },
            Stmt::GCode {
                template: "G0 X0".into(),
            },
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
        let p = program(vec![Stmt::Wait {
            cond: Expr::Status("idle".into()),
        }]);
        let error = run_macro(&p, vec![], &mut rt, &cancel, &mut log, &mut holder).unwrap_err();
        assert!(matches!(error, RosettaError::Runtime { .. }));
    }

    #[test]
    fn abort_yields_aborted_outcome_not_error() {
        let p = program(vec![
            Stmt::Abort {
                reason: Expr::Str("no tool".into()),
            },
            Stmt::GCode {
                template: "G1 X1".into(),
            },
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
            body: vec![Stmt::GCode {
                template: "G4 P1".into(),
            }],
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
            Stmt::Call {
                name: "double".into(),
                args: vec![Expr::Num(21.0)],
            },
            Stmt::SetVar {
                name: "result".into(),
                value: Expr::Var("return".into()),
            },
            Stmt::If {
                cond: bin(BinOp::Eq, Expr::Var("result".into()), Expr::Num(42.0)),
                then: vec![Stmt::GCode {
                    template: "M999".into(),
                }],
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
            body: vec![Stmt::Return {
                value: Some(bin(BinOp::Mul, Expr::Var("v".into()), Expr::Num(2.0))),
            }],
        }];
        let (outcome, rt) = run(&p, vec![], HashMap::new(), None);
        assert!(outcome.is_ok());
        assert_eq!(rt.sent, vec!["M999"]);
    }

    #[test]
    fn invoke_transform_uses_bridge_and_exposes_return() {
        let p = program(vec![
            Stmt::InvokeTransform {
                id: "Rotate".into(),
                args: vec![Expr::Num(90.0)],
            },
            Stmt::GCode {
                template: "M100 {return}".into(),
            },
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
            body: vec![Stmt::Comment {
                text: "tick".into(),
            }],
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
        assert!(matches!(
            outcome.unwrap_err(),
            RosettaError::Validation { .. }
        ));
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
            body: vec![Stmt::GCode {
                template: "G1 X{c}".into(),
            }],
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
        e = Expr::Unary {
            op: UnOp::Not,
            arg: Box::new(Expr::Bool(false)),
        };
        assert_eq!(scope.eval(&e).unwrap(), Value::Bool(true));
    }

    #[test]
    fn interpolation_requires_balance_and_escapes() {
        // An unterminated placeholder must be a hard parse error...
        let p = program(vec![Stmt::GCode {
            template: "G4 {x".into(),
        }]);
        // ...while a doubled brace is a literal escape and `{` inside plain
        // text that is not a `{name}` placeholder stays untouched only when
        // it is escaped; here the stray `{x` after the escape is unterminated
        // too, so this program must also fail.
        let q = program(vec![Stmt::GCode {
            template: "G4 {{literal {x".into(),
        }]);
        let mut r = program(vec![Stmt::GCode {
            template: "G4 {x}".into(),
        }]);
        r.params = vec![ParamDecl {
            name: "x".into(),
            kind: ValueKind::Number,
            default: Some(Expr::Num(1.0)),
            description: None,
        }];
        let (outcome, _rt) = run(&p, vec![], HashMap::new(), None);
        assert!(outcome.is_err(), "unterminated placeholder must fail");
        let (outcome, _rt) = run(&q, vec![], HashMap::new(), None);
        assert!(
            outcome.is_err(),
            "escape must not hide an unterminated brace"
        );
        let (outcome, rt) = run(&r, vec![], HashMap::new(), None);
        assert!(outcome.is_ok());
        assert_eq!(rt.sent, vec!["G4 1"]);
    }
}
