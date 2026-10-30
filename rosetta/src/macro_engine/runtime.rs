//! The host seam of the Macro Engine (ТЗ №2 §8, §9): everything the
//! interpreter is allowed to know about the outside world.
//!
//! Keeping the machine behind the [`HostRuntime`] trait makes the engine
//! testable without a controller (canned status maps below) and lets an
//! application wrap its own `CncApi`/transport asynchronously while this
//! crate stays synchronous (documented deviation, see crate docs).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::{RosettaError, RosettaResult};
use crate::ir::macro_ir::Value;

/// Coarse machine state as seen by the engine (`MAC` §6.3 state machine and
/// the safety gate §9). `Default` is `Idle` — a test/demo runtime starts in
/// the "powered, safe to run" state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineState {
    Disconnected,
    #[default]
    /// Powered, no active job — safe to start a macro.
    Idle,
    /// A program/macro is already streaming; macros queue behind it.
    Running,
    Paused,
    /// Controller alarm — starting anything is forbidden (§9.2).
    Alarm,
    /// Emergency stop engaged — highest priority kill (§9.1).
    EStop,
    /// Unknown/other state reported by an adapter.
    Other,
}

impl MachineState {
    /// Whether a macro run may start from this state (ТЗ №2 §9.2: reject
    /// `Alarm`/`Disconnected`, and E-Stop obviously).
    pub fn allows_start(self) -> bool {
        matches!(self, MachineState::Idle | MachineState::Running | MachineState::Paused)
    }

    /// Whether an already-running engine must abort immediately (§9.1:
    /// E-Stop has the highest priority and kills the task).
    pub fn must_abort(self) -> bool {
        matches!(self, MachineState::EStop | MachineState::Alarm)
    }
}

/// Shared, thread-safe cancellation flag — the synchronous stand-in for the
/// ТЗ `CancellationToken` / `tokio::select!` (ТЗ №2 §6.4). Any other task
/// (the UI "Stop" button) flips it; the interpreter checks it between steps.
#[derive(Debug, Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    /// A fresh token in the "not cancelled" state.
    pub fn new() -> Self {
        CancellationToken(Arc::new(AtomicBool::new(false)))
    }

    /// Request cancellation of the current run.
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    /// Whether cancellation was requested.
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// One line of the `[MACRO]` colored log stream (ТЗ №2 §10). The engine
/// produces these; the UI decides colors and routing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroLogLine {
    /// Macro id this line belongs to.
    pub macro_id: String,
    /// `1`-based step counter and total, when the total is statically known
    /// (`Some` for straight-line bodies; loop bodies report `None` totals).
    pub step: Option<(u64, Option<u64>)>,
    /// What happened: `"G91 G38.2 Z-50 F100 -> ok"`, `"delay 500ms"`, …
    pub message: String,
    /// Severity knob for the UI: `info` / `warn` / `error`.
    pub level: MacroLogLevel,
}

/// Log severities of [`MacroLogLine`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroLogLevel {
    Info,
    Warn,
    Error,
}

/// Terminal result of one macro run (`MAC` §6.3 `Done`/`Aborted` states).
#[derive(Debug, Clone, PartialEq)]
pub enum RunOutcome {
    /// Body finished normally; carries the top-level `return` value if any.
    Completed { value: Option<Value> },
    /// The macro aborted itself via `Stmt::Abort`.
    Aborted { reason: String },
    /// External cancellation via [`CancellationToken`] or host "Stop".
    Cancelled,
}

/// Everything the engine may ask of the machine. Implementations must make
/// [`HostRuntime::send_command`] *blocking* — the command is executed, not
/// merely queued (ТЗ №2 §8: `execute_mdi_blocking`).
pub trait HostRuntime {
    /// Send one flat G/M/S command and block until the controller confirms
    /// execution. The returned string is the controller echo/response.
    fn send_command(&mut self, command: &str) -> RosettaResult<String>;

    /// Read one status field for `Expr::Status` (`"probe_trig"`, `"pos.z"`,
    /// `"state"`, `"tool"` …). Unknown fields must return an error so typos
    /// surface early (ТЗ №1 principle 4), not `Null`.
    fn status_field(&mut self, key: &str) -> RosettaResult<Value>;

    /// Sleep the given amount of milliseconds (for `Stmt::Delay`). A runtime
    /// may treat this as a no-op in dry runs.
    fn sleep_ms(&mut self, ms: u64);

    /// Current coarse machine state; consulted by `Wait`, dry-run checks and
    /// the abort-on-E-Stop rule (§9.1).
    fn machine_state(&mut self) -> MachineState;
}

/// Dry-run runtime (ТЗ №2 §9.4, mandatory since v1): executes macro *logic*
/// while every motion command is only recorded with a `[DRY RUN]` prefix and
/// sleeps are skipped. `status_field` still reads real state so the control
/// flow is exercised exactly as it would be live.
pub struct DryRunRuntime<'a> {
    /// The live runtime whose status/state channels stay active.
    pub inner: &'a mut dyn HostRuntime,
    /// Commands that would have been sent, in order — inspect after the run.
    pub recorded: Vec<String>,
}

impl<'a> DryRunRuntime<'a> {
    /// Wrap a live runtime for logic-only execution.
    pub fn new(inner: &'a mut dyn HostRuntime) -> Self {
        DryRunRuntime { inner, recorded: Vec::new() }
    }
}

impl HostRuntime for DryRunRuntime<'_> {
    fn send_command(&mut self, command: &str) -> RosettaResult<String> {
        self.recorded.push(format!("[DRY RUN] {command}"));
        Ok("ok".to_owned())
    }

    fn status_field(&mut self, key: &str) -> RosettaResult<Value> {
        self.inner.status_field(key)
    }

    fn sleep_ms(&mut self, _ms: u64) {
        // A dry run must not actually wait.
    }

    fn machine_state(&mut self) -> MachineState {
        self.inner.machine_state()
    }
}

/// Command-collecting runtime used by the *offline generative expander*
/// (`CORE-M-04`): the interpreter logic runs, but instead of talking to a
/// machine every sent command becomes an output line, delays become dwells,
/// and status reads are refused — status waiting is a reactive-only feature
/// (capability matrix row "Wait / GRBL генеративный ❌").
pub struct EmitRuntime {
    /// Emitted flat lines in execution order.
    pub lines: Vec<String>,
}

impl EmitRuntime {
    /// A fresh collector.
    pub fn new() -> Self {
        EmitRuntime { lines: Vec::new() }
    }
}

impl Default for EmitRuntime {
    fn default() -> Self {
        EmitRuntime::new()
    }
}

impl HostRuntime for EmitRuntime {
    fn send_command(&mut self, command: &str) -> RosettaResult<String> {
        self.lines.push(command.trim().to_owned());
        Ok("ok".to_owned())
    }

    fn status_field(&mut self, key: &str) -> RosettaResult<Value> {
        Err(RosettaError::Capability {
            backend: "generative expansion".into(),
            construct: format!("status:{key}"),
            detail: "offline expansion cannot read machine status".into(),
        })
    }

    fn sleep_ms(&mut self, ms: u64) {
        // Materialize the delay as an explicit dwell so the flat program is
        // faithful to the macro semantics.
        self.lines.push(format!("G4 P{}", crate::util::fmt_num(ms as f64)));
    }

    fn machine_state(&mut self) -> MachineState {
        MachineState::Idle
    }
}

/// Test/demo runtime backed by canned status values and an in-memory echo
/// log (ТЗ №2 §12: the engine must run without hardware in CI). Lives in the
/// library so integration tests and embedders can use it too.
#[derive(Debug, Default)]
pub struct ScriptedRuntime {
    /// Status fields the runtime exposes; missing key => error.
    pub status: HashMap<String, Value>,
    /// Every command that was "executed".
    pub sent: Vec<String>,
    /// Machine state reported by [`HostRuntime::machine_state`].
    pub state: MachineState,
}

impl ScriptedRuntime {
    /// Build a runtime serving `status` at `state` (default `Idle`).
    pub fn new(status: HashMap<String, Value>, state: MachineState) -> Self {
        ScriptedRuntime { status, sent: Vec::new(), state }
    }
}

impl HostRuntime for ScriptedRuntime {
    fn send_command(&mut self, command: &str) -> RosettaResult<String> {
        self.sent.push(command.to_owned());
        Ok("ok".to_owned())
    }

    fn status_field(&mut self, key: &str) -> RosettaResult<Value> {
        self.status
            .get(key)
            .cloned()
            .ok_or_else(|| RosettaError::Runtime { reason: format!("unknown status field \"{key}\"") })
    }

    fn sleep_ms(&mut self, _ms: u64) {}

    fn machine_state(&mut self) -> MachineState {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_state_gates() {
        assert!(!MachineState::Alarm.allows_start());
        assert!(!MachineState::Disconnected.allows_start());
        assert!(MachineState::Idle.allows_start());
        assert!(MachineState::EStop.must_abort());
        assert!(!MachineState::Paused.must_abort());
    }

    #[test]
    fn cancel_token_is_shareable() {
        let token = CancellationToken::new();
        let clone = token.clone();
        assert!(!clone.is_cancelled());
        token.cancel();
        assert!(clone.is_cancelled());
    }

    #[test]
    fn dry_run_records_instead_of_sending() {
        let mut inner = ScriptedRuntime::default();
        inner.status.insert("probe_trig".into(), Value::Num(1.0));
        let mut dry = DryRunRuntime::new(&mut inner);
        dry.send_command("G0 X1").unwrap();
        assert_eq!(dry.status_field("probe_trig").unwrap(), Value::Num(1.0));
        assert_eq!(dry.recorded, vec!["[DRY RUN] G0 X1"]);
        assert!(inner.sent.is_empty());
    }

    #[test]
    fn emit_runtime_materializes_delays_and_refuses_status() {
        let mut emit = EmitRuntime::new();
        emit.send_command("G1 Z-5 F100").unwrap();
        emit.sleep_ms(250);
        assert_eq!(emit.lines, vec!["G1 Z-5 F100", "G4 P250"]);
        assert!(emit.status_field("pos.x").is_err());
    }
}
