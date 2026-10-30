//! The safety pipeline (ТЗ №2 §9, contract `MAC-C-07`): nothing runs until
//! the machine state, the confirmation policy and the capability matrix all
//! agree it is legal.
//!
//! [`execute`] is the single sanctioned path into the interpreter — the
//! guarantee of `MAC-C-07` ("never run what failed `CORE-M-03`") lives here,
//! not in the interpreter itself. [`prestart`] is exposed separately so a UI
//! can query "may I enable this button / show this modal?" without running.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::error::{RosettaError, RosettaResult};
use crate::ir::capability::{BackendType, CapabilityMatrix, CapabilityRegistry, Feature, Support};
use crate::ir::macro_ir::{Expr, MacroProgram, Stmt, Trigger};
use crate::macro_engine::interpreter::{self, TransformHook};
use crate::macro_engine::runtime::{
    CancellationToken, HostRuntime, MachineState, MacroLogLine, MacroLogLevel, RunOutcome,
};

/// Outcome of the pre-start safety interrogation (`MAC` §9.2/§9.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SafetyReport {
    /// Whether the run may start at all right now.
    pub allowed: bool,
    /// Whether the host must show a confirmation dialog first (§9.3:
    /// `meta.require_confirm` or the matched trigger's flag).
    pub requires_confirm: bool,
    /// Human-readable reasons (denied states, pending confirmation) that a
    /// UI can surface verbatim.
    pub notes: Vec<String>,
}

/// Check the *machine-side* preconditions for running `program` through
/// `trigger` while the controller reports `state` (`MAC` §9.2):
/// `Disconnected`/`Alarm`/`EStop` deny the start; anything else allows it.
/// Confirmation is purely declarative here — the engine never blocks on a
/// dialog itself, it only reports that one is required (§9.3: the UI owns
/// the modal).
pub fn prestart(program: &MacroProgram, trigger: Option<&Trigger>, state: MachineState) -> SafetyReport {
    let mut notes = Vec::new();
    let allowed = match state {
        MachineState::Disconnected => {
            notes.push("machine is not connected".to_owned());
            false
        }
        MachineState::Alarm => {
            notes.push("controller is in ALARM; reset before running macros".to_owned());
            false
        }
        MachineState::EStop => {
            notes.push("emergency stop is engaged".to_owned());
            false
        }
        other => {
            if other == MachineState::Running {
                notes.push("a job is streaming; the macro interleaves when the host queues it".to_owned());
            }
            true
        }
    };
    let requires_confirm = program.meta.require_confirm
        || trigger.map(Trigger::requires_confirm).unwrap_or(false);
    if requires_confirm && allowed {
        notes.push(format!("macro \"{}\" requires confirmation", program.meta.id));
    }
    SafetyReport { allowed, requires_confirm, notes }
}

/// Collect the IR [`Feature`]s a program actually uses — the dynamic input
/// of the capability gate (`CORE-M-03`). The scan is deliberately coarse
/// (presence, not frequency): one unsupported construct forbids the run.
pub fn scan_features(program: &MacroProgram) -> BTreeSet<Feature> {
    let mut out = BTreeSet::new();
    for stmt in &program.body {
        scan_stmt(stmt, &mut out);
    }
    for sub in &program.subs {
        // Any subroutine means the Subprograms construct is in play.
        out.insert(Feature::Subprograms);
        for stmt in &sub.body {
            scan_stmt(stmt, &mut out);
        }
    }
    if has_recursion(program) {
        out.insert(Feature::Recursion);
    }
    out
}

/// Walk one statement (and its nested blocks/expressions) adding features.
fn scan_stmt(stmt: &Stmt, out: &mut BTreeSet<Feature>) {
    match stmt {
        Stmt::GCode { template } => {
            out.insert(Feature::Sequence);
            // A `{name}` placeholder (not the `{{` literal escape) reads a
            // variable, i.e. needs the Variables capability.
            if template_uses_vars(template) {
                out.insert(Feature::Variables);
            }
        }
        Stmt::Delay { ms } => {
            out.insert(Feature::Delay);
            scan_expr(ms, out);
        }
        Stmt::Wait { cond } => {
            out.insert(Feature::WaitStatus);
            scan_expr(cond, out);
        }
        Stmt::SetVar { value, .. } => {
            out.insert(Feature::Variables);
            scan_expr(value, out);
        }
        Stmt::If { cond, then, otherwise } => {
            out.insert(Feature::IfElse);
            scan_expr(cond, out);
            for s in then.iter().chain(otherwise) {
                scan_stmt(s, out);
            }
        }
        Stmt::While { cond, body } => {
            out.insert(Feature::WhileRepeat);
            scan_expr(cond, out);
            for s in body {
                scan_stmt(s, out);
            }
        }
        Stmt::Repeat { count, body } => {
            out.insert(Feature::WhileRepeat);
            scan_expr(count, out);
            for s in body {
                scan_stmt(s, out);
            }
        }
        Stmt::ForEach { iter, body, .. } => {
            out.insert(Feature::ForeachArrays);
            scan_expr(iter, out);
            for s in body {
                scan_stmt(s, out);
            }
        }
        Stmt::Call { args, .. } => {
            out.insert(Feature::Subprograms);
            for arg in args {
                scan_expr(arg, out);
            }
        }
        Stmt::InvokeTransform { args, .. } => {
            out.insert(Feature::InvokeTransform);
            for arg in args {
                scan_expr(arg, out);
            }
        }
        Stmt::Return { value } => {
            if let Some(expr) = value {
                scan_expr(expr, out);
            }
        }
        Stmt::Abort { reason } => scan_expr(reason, out),
        Stmt::Comment { .. } => {
            out.insert(Feature::Sequence);
        }
    }
}

/// Walk one expression: status reads need `WaitStatus`, arrays/ranges/index
/// need `ForeachArrays`, variable reads need `Variables`.
fn scan_expr(expr: &Expr, out: &mut BTreeSet<Feature>) {
    match expr {
        Expr::Num(_) | Expr::Str(_) | Expr::Bool(_) => {}
        Expr::Var(_) => {
            out.insert(Feature::Variables);
        }
        Expr::Status(_) => {
            out.insert(Feature::WaitStatus);
        }
        Expr::Array(items) => {
            out.insert(Feature::ForeachArrays);
            for item in items {
                scan_expr(item, out);
            }
        }
        Expr::Index(base, index) => {
            out.insert(Feature::ForeachArrays);
            scan_expr(base, out);
            scan_expr(index, out);
        }
        Expr::Range { start, end, step } => {
            out.insert(Feature::ForeachArrays);
            scan_expr(start, out);
            scan_expr(end, out);
            if let Some(step) = step {
                scan_expr(step, out);
            }
        }
        Expr::Bin { lhs, rhs, .. } => {
            scan_expr(lhs, out);
            scan_expr(rhs, out);
        }
        Expr::Unary { arg, .. } => scan_expr(arg, out),
        Expr::CallExpr { args, .. } => {
            for arg in args {
                scan_expr(arg, out);
            }
        }
    }
}

/// Whether a G-code template contains a real placeholder `{name}` as opposed
/// to only `{{`/`}}` literal escapes.
fn template_uses_vars(template: &str) -> bool {
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '{' {
            if i + 1 < chars.len() && chars[i + 1] == '{' {
                i += 2; // escaped literal brace
                continue;
            }
            // Any `{...}` reaches a variable read in the interpreter.
            return true;
        }
        i += 1;
    }
    false
}

/// Detect recursive subroutine usage: a call graph cycle among `subs`, or a
/// body-level `Call` that (transitively) re-enters its own subroutine.
fn has_recursion(program: &MacroProgram) -> bool {
    // name → direct callee sub names
    let graph: Vec<(String, Vec<String>)> = program
        .subs
        .iter()
        .map(|sub| {
            let callees = sub
                .body
                .iter()
                .filter_map(|s| match s {
                    Stmt::Call { name, .. } => Some(name.clone()),
                    _ => None,
                })
                .filter(|name| program.find_sub(name).is_some())
                .collect();
            (sub.name.clone(), callees)
        })
        .collect();
    // DFS cycle detection on such a small graph (subs are hand-written).
    fn walk(
        node: &str,
        graph: &[(String, Vec<String>)],
        stack: &mut Vec<String>,
        seen: &mut BTreeSet<String>,
    ) -> bool {
        if stack.iter().any(|s| s == node) {
            return true;
        }
        if !seen.insert(node.to_owned()) {
            return false;
        }
        stack.push(node.to_owned());
        if let Some((_, callees)) = graph.iter().find(|(name, _)| name == node) {
            for callee in callees {
                if walk(callee, graph, stack, seen) {
                    return true;
                }
            }
        }
        stack.pop();
        false
    }
    graph
        .iter()
        .any(|(name, _)| walk(name, &graph, &mut Vec::new(), &mut BTreeSet::new()))
}

/// The complete safety-capped run (`MAC-C-07`): machine-state gate, then the
/// capability gate over every used [`Feature`] for `(backend, class)`, then
/// the interpreter. `Ok(RunOutcome)` carries completed/aborted/cancelled;
/// `Err` means the run never started or died on a transport failure.
///
/// The generative offline path (`macro_engine::generative`) calls the same
/// feature gate with [`crate::ir::macro_ir::MacroClass::Generative`] before
/// expansion, so both entry points honor `CORE-M-03` identically.
pub fn execute<'h>(
    program: &MacroProgram,
    args: Vec<crate::ir::macro_ir::Value>,
    rt: &mut dyn HostRuntime,
    cancel: &CancellationToken,
    log: &mut dyn FnMut(MacroLogLine),
    transform: &mut Option<&'h mut TransformHook<'h>>,
    matrix: &CapabilityMatrix,
    backend: BackendType,
) -> RosettaResult<RunOutcome> {
    // §9.2: refuse to start against a hostile machine state.
    let state = rt.machine_state();
    let report = prestart(program, None, state);
    if !report.allowed {
        return Err(RosettaError::Runtime {
            reason: report.notes.join("; "),
        });
    }
    // CORE-M-03 / MAC-C-03: reject constructs the backend cannot express.
    let mut blocked: Option<(Feature, Support)> = None;
    for feature in scan_features(program) {
        let support = matrix.supports_feature(backend, program.meta.class, feature);
        if !support.supported() {
            blocked = Some((feature, support));
            break;
        }
    }
    if let Some((feature, support)) = blocked {
        let detail = match &support {
            Support::Partial(why) => why.clone(),
            _ => "not supported on this backend".to_owned(),
        };
        (log)(MacroLogLine {
            macro_id: program.meta.id.clone(),
            step: None,
            message: format!("capability gate: {feature:?} — {detail}"),
            level: MacroLogLevel::Error,
        });
        return Err(RosettaError::Capability {
            backend: backend.label().to_owned(),
            construct: format!("{feature:?}"),
            detail,
        });
    }
    // Structural check even for programs assembled in memory (`CORE-M-02`).
    program.validate()?;
    interpreter::run_macro(program, args, rt, cancel, log, transform)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::capability::CapabilityMatrix;
    use crate::ir::macro_ir::{MacroClass, MacroMeta, Value};
    use crate::macro_engine::runtime::ScriptedRuntime;

    /// Program with an explicit class and body.
    fn program(class: MacroClass, body: Vec<Stmt>) -> MacroProgram {
        MacroProgram {
            meta: MacroMeta {
                id: "t".into(),
                name: "T".into(),
                icon: None,
                category: None,
                class,
                require_confirm: false,
            },
            params: Vec::new(),
            locals: Vec::new(),
            subs: Vec::new(),
            triggers: Vec::new(),
            body,
        }
    }

    #[test]
    fn prestart_blocks_unsafe_states() {
        let p = program(MacroClass::Reactive, vec![]);
        assert!(!prestart(&p, None, MachineState::Alarm).allowed);
        assert!(!prestart(&p, None, MachineState::Disconnected).allowed);
        assert!(!prestart(&p, None, MachineState::EStop).allowed);
        assert!(prestart(&p, None, MachineState::Idle).allowed);
        // Confirmation is declarative, never blocking.
        let trigger = Trigger::Button { require_confirm: true };
        assert!(prestart(&p, Some(&trigger), MachineState::Idle).requires_confirm);
    }

    #[test]
    fn execute_refuses_generative_wait_on_grbl() {
        // MAC-C-07: a construct that fails CORE-M-03 must not reach the
        // interpreter at all.
        let p = program(
            MacroClass::Generative,
            vec![Stmt::Wait { cond: Expr::Status("probe_trig".into()) }],
        );
        let mut rt = ScriptedRuntime::default();
        let cancel = CancellationToken::new();
        let error = execute(
            &p,
            vec![],
            &mut rt,
            &cancel,
            &mut |_| {},
            &mut None,
            &CapabilityMatrix::new(),
            BackendType::Grbl,
        )
        .unwrap_err();
        assert!(matches!(error, RosettaError::Capability { ref construct, .. } if construct == "WaitStatus"));
        assert!(rt.sent.is_empty());
    }

    #[test]
    fn execute_allows_reactive_wait_on_grbl_and_runs() {
        let mut p = program(MacroClass::Reactive, vec![Stmt::GCode { template: "G0 X1".into() }]);
        p.meta.require_confirm = false;
        let mut rt = ScriptedRuntime::default();
        let cancel = CancellationToken::new();
        let outcome = execute(
            &p,
            vec![Value::Num(1.0)],
            &mut rt,
            &cancel,
            &mut |_| {},
            &mut None,
            &CapabilityMatrix::new(),
            BackendType::Grbl,
        )
        .unwrap();
        assert!(matches!(outcome, RunOutcome::Completed { value: None }));
        assert_eq!(rt.sent, vec!["G0 X1"]);
    }

    #[test]
    fn scan_flags_recursion_and_arrays() {
        use crate::ir::macro_ir::Subroutine;
        let mut p = program(MacroClass::Reactive, vec![Stmt::ForEach {
            var: "i".into(),
            iter: Expr::Range {
                start: Box::new(Expr::Num(0.0)),
                end: Box::new(Expr::Num(3.0)),
                step: None,
            },
            body: vec![Stmt::Comment { text: "tick".into() }],
        }]);
        p.subs = vec![Subroutine {
            name: "a".into(),
            params: Vec::new(),
            description: None,
            body: vec![Stmt::Call { name: "a".into(), args: Vec::new() }],
        }];
        let features = scan_features(&p);
        assert!(features.contains(&Feature::ForeachArrays));
        assert!(features.contains(&Feature::Recursion));
        assert!(features.contains(&Feature::Subprograms));
    }
}
