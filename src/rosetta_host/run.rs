//! Macro run orchestration for the web/Tauri backend (T26 plan §3.1, §3.5).
//!
//! One run = one OS thread (the Rosetta interpreter is synchronous and
//! blocking; the engine docs prescribe exactly this host-side pattern) plus a
//! [`RunRecord`] in the shared [`RunRegistry`]. The web handler starts a run,
//! returns its id immediately, and the UI tails the log through
//! `GET /api/macros/run/{run_id}` (offset polling — the same pattern as the
//! rest of this backend's status endpoints).
//!
//! Cancellation is cooperative and reuses the controller's existing "Stop"
//! flag: the token flips the record state, and
//! [`crate::cnc_controller::ControllerHandle::execute_blocking`] steps check
//! the shared `command_cancel_requested` flag, so the same Stop button aborts
//! MDI commands, program streaming *and* macros uniformly.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::Builder;
use std::time::Instant;

use rosetta::error::RosettaResult;
use rosetta::ir::capability::{BackendType, CapabilityMatrix};
use rosetta::ir::macro_ir::{MacroId, Value};
use rosetta::macro_engine::registry::MacroRegistry;
use rosetta::macro_engine::runtime::{
    CancellationToken, DryRunRuntime, MacroLogLevel, MacroLogLine, RunOutcome,
};
use rosetta::macro_engine::{Invoker, MacroInvoker};
use serde::Serialize;

use super::runtime::ControllerRuntime;
use crate::cnc_controller::ControllerHandle;

/// Monotonic run id source (unique within the process; ids are short strings
/// like `r7` for the UI).
static RUN_COUNTER: AtomicU64 = AtomicU64::new(1);

/// One journal line as served to the UI (the JSON shape is the wire contract).
#[derive(Debug, Clone, Serialize)]
pub struct RunLogLine {
    /// 0-based index used as the client's read offset.
    pub index: usize,
    /// `info` / `warn` / `error`, mirroring `MacroLogLevel`.
    pub level: &'static str,
    /// Optional `step/total` progress, `"3/9"` style.
    pub step: Option<String>,
    /// The engine message.
    pub message: String,
}

/// Lifecycle of a macro run as seen by the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Interpreter active on the run thread.
    Running,
    /// Finished normally.
    Completed,
    /// Macro aborted itself (`Stmt::Abort`).
    Aborted,
    /// Cancelled via the token or the machine Stop flag.
    Cancelled,
    /// Failed to start or died on a controller error.
    Failed,
}

impl RunState {
    /// Whether the run may still append log lines.
    pub fn active(self) -> bool {
        matches!(self, RunState::Running)
    }
}

/// Mutable state of one run, guarded by its own mutex so the UI can tail the
/// log while the worker thread appends to it.
pub struct RunRecord {
    /// Run id (registry key).
    pub run_id: String,
    /// Macro id this run executes.
    pub macro_id: String,
    /// Whether the run was started as a dry run (ТЗ №2 §9.4).
    pub dry: bool,
    /// Current lifecycle state.
    pub state: RunState,
    /// Engine log lines collected so far.
    pub lines: Vec<RunLogLine>,
    /// Terminal outcome summary for the UI status line.
    pub result: Option<String>,
    /// When the run reached a terminal state (eviction ordering).
    pub finished_at: Instant,
    /// Cancellation handle so the cancel endpoint can flip the token.
    cancel: CancellationToken,
    /// Controller-level stop flag: raised together with the token so the
    /// blocking machine step currently in flight aborts too (the adapter
    /// polls the same flag the UI "Stop" button sets).
    stop_flag: Arc<std::sync::atomic::AtomicBool>,
}

/// Snapshot of a run for the JSON API (`GET /api/macros/run/{run_id}`).
#[derive(Debug, Clone, Serialize)]
pub struct RunStatus {
    /// Run id.
    pub run_id: String,
    pub macro_id: String,
    pub dry: bool,
    pub state: RunState,
    pub result: Option<String>,
    /// Log lines with `index >= from`.
    pub lines: Vec<RunLogLine>,
    /// Next offset the client should request.
    pub next_offset: usize,
}

/// Shared table of active and recently finished runs.
#[derive(Default)]
pub struct RunRegistry {
    records: Mutex<HashMap<String, Arc<Mutex<RunRecord>>>>,
}

/// Finished runs kept for tailing before eviction; active runs are never
/// evicted.
const HISTORY_LIMIT: usize = 32;

impl RunRegistry {
    /// Empty registry.
    pub fn new() -> Self {
        RunRegistry::default()
    }

    /// Inserts a fresh record and returns its handle.
    fn insert(&self, record: RunRecord) -> Arc<Mutex<RunRecord>> {
        let key = record.run_id.clone();
        let shared = Arc::new(Mutex::new(record));
        let mut table = self.records.lock().expect("run registry lock");
        table.insert(key, shared.clone());
        // Evict the oldest finished runs so a polling UI cannot grow memory.
        if table.len() > HISTORY_LIMIT {
            let mut finished: Vec<(Instant, String)> = table
                .iter()
                .filter(|(_, value)| !value.lock().expect("run record lock").state.active())
                .map(|(key, value)| {
                    (
                        value.lock().expect("run record lock").finished_at,
                        key.clone(),
                    )
                })
                .collect();
            finished.sort_by_key(|(at, _)| *at);
            let mut cursor = 0usize;
            while table.len() > HISTORY_LIMIT && cursor < finished.len() {
                table.remove(&finished[cursor].1);
                cursor += 1;
            }
        }
        shared
    }

    /// Requests cancellation of a run by id (returns false when unknown).
    pub fn cancel(&self, run_id: &str) -> bool {
        let record = self
            .records
            .lock()
            .expect("run registry lock")
            .get(run_id)
            .cloned();
        match record {
            Some(record) => {
                let mut record = record.lock().expect("run record lock");
                if record.state.active() {
                    record.cancel.cancel();
                    // Raise the controller-level stop flag too, so a blocking
                    // Execute step currently in flight is abandoned as well.
                    record.stop_flag.store(true, Ordering::Release);
                    record.state = RunState::Cancelled;
                }
                true
            }
            None => false,
        }
    }

    /// Reads the run status with log tailing from `from`.
    pub fn status(&self, run_id: &str, from: usize) -> Option<RunStatus> {
        let record = self
            .records
            .lock()
            .expect("run registry lock")
            .get(run_id)
            .cloned()?;
        let record = record.lock().expect("run record lock");
        Some(RunStatus {
            run_id: record.run_id.clone(),
            macro_id: record.macro_id.clone(),
            dry: record.dry,
            state: record.state,
            result: record.result.clone(),
            lines: record
                .lines
                .iter()
                .filter(|line| line.index >= from)
                .cloned()
                .collect(),
            next_offset: record.lines.len(),
        })
    }

    /// Whether any run is currently active. Guards against overlapping runs:
    /// the blocking Execute path would interleave two macro step streams.
    pub fn has_active(&self) -> bool {
        self.records
            .lock()
            .expect("run registry lock")
            .values()
            .any(|record| record.lock().expect("run record lock").state.active())
    }
}

/// Starts one macro run on a dedicated OS thread and returns its id.
///
/// * `registry` — programs built from the current `user_macros` rows;
/// * `backend` — capability target of the connected machine;
/// * `handle` — cloned controller handle for the run thread;
/// * `dry` — wrap the runtime in [`DryRunRuntime`] (logic and status reads
///   stay live, motion commands are only recorded);
/// * `args` — positional macro parameters (JSON numbers/strings from the UI).
///
/// Errors only on failures *before* the thread spawns (unknown macro, hostile
/// machine state for a live run). Interpreter/controller failures land in the
/// run record and are visible through the log stream.
pub fn start_run(
    registry: MacroRegistry,
    runs: &RunRegistry,
    backend: BackendType,
    handle: ControllerHandle,
    macro_id: &str,
    dry: bool,
    args: Vec<Value>,
    matrix: CapabilityMatrix,
) -> Result<String, String> {
    if runs.has_active() {
        return Err("a macro run is already active; cancel it first".into());
    }
    let id: MacroId = macro_id.to_owned();
    if registry.get(&id).is_none() {
        return Err(format!("macro \"{macro_id}\" is not registered"));
    }
    // Refuse live runs from a hostile machine state before spawning (§9.2);
    // dry runs cannot move the machine, so they are always allowed.
    let state = ControllerRuntime::machine_state_of(&handle.snapshot());
    if !dry && !state.allows_start() {
        return Err(format!(
            "machine state {state:?} does not allow starting a macro"
        ));
    }

    let run_id = format!("r{}", RUN_COUNTER.fetch_add(1, Ordering::Relaxed));
    let cancel = CancellationToken::new();
    // Give the cancellation path access to the controller stop flag: the
    // runtime's steps check it between commands; reset it first so a previous
    // cancelled machine command does not poison the new run.
    let stop_flag = handle.cancel_flag();
    if !dry {
        handle.reset_cancellation();
    }
    let shared = runs.insert(RunRecord {
        run_id: run_id.clone(),
        macro_id: macro_id.to_owned(),
        dry,
        state: RunState::Running,
        lines: Vec::new(),
        result: None,
        finished_at: Instant::now(),
        cancel: cancel.clone(),
        stop_flag,
    });

    let thread_handle = handle;
    let thread_run_id = run_id.clone();
    Builder::new()
        .name(format!("rosetta-macro-{thread_run_id}"))
        .spawn(move || {
            let outcome = execute_run(
                &registry,
                &matrix,
                backend,
                thread_handle,
                &id,
                dry,
                args,
                &shared,
                &cancel,
            );
            let mut record = shared.lock().expect("run record lock");
            let (state, note) = classify(outcome, record.cancel.is_cancelled());
            // A UI-side cancel already set Cancelled; keep that state and
            // attach the engine's final note for the status line.
            if !record.state.active() {
                record.result = Some(note);
            } else {
                record.state = state;
                record.result = Some(note);
            }
            record.finished_at = Instant::now();
        })
        .map_err(|error| format!("cannot spawn macro thread: {error}"))?;

    Ok(run_id)
}

/// Maps the engine outcome onto the UI-visible (state, note) pair.
fn classify(outcome: RosettaResult<RunOutcome>, cancelled: bool) -> (RunState, String) {
    match outcome {
        Ok(RunOutcome::Completed { value: Some(value) }) if value != Value::Null => (
            RunState::Completed,
            format!("completed, returned {value:?}"),
        ),
        Ok(RunOutcome::Completed { .. }) => (RunState::Completed, "completed".to_string()),
        Ok(RunOutcome::Aborted { reason }) => (RunState::Aborted, format!("aborted: {reason}")),
        Ok(RunOutcome::Cancelled) => (RunState::Cancelled, "cancelled".to_string()),
        // The engine reports cancellation as a Runtime error when the flag
        // was flipped mid-step; present it as Cancelled, not Failed.
        Err(_) if cancelled => (RunState::Cancelled, "cancelled".to_string()),
        Err(error) => (RunState::Failed, error.to_string()),
    }
}

/// Runs the interpreter to completion, appending engine log lines to the
/// shared record as they arrive.
fn execute_run(
    registry: &MacroRegistry,
    matrix: &CapabilityMatrix,
    backend: BackendType,
    handle: ControllerHandle,
    id: &MacroId,
    dry: bool,
    args: Vec<Value>,
    record: &Arc<Mutex<RunRecord>>,
    cancel: &CancellationToken,
) -> RosettaResult<RunOutcome> {
    let mut runtime = ControllerRuntime::new(handle);
    let mut sink = |line: MacroLogLine| {
        let mut guard = record.lock().expect("run record lock");
        let index = guard.lines.len();
        guard.lines.push(RunLogLine {
            index,
            level: match line.level {
                MacroLogLevel::Info => "info",
                MacroLogLevel::Warn => "warn",
                MacroLogLevel::Error => "error",
            },
            step: line.step.map(|(step, total)| match total {
                Some(total) => format!("{step}/{total}"),
                None => format!("{step}"),
            }),
            message: line.message,
        });
    };

    // The transform bridge slot is unused on this path for now: a macro that
    // calls InvokeTransform is refused by the capability gate with a clear
    // message instead of silently doing nothing.
    if dry {
        let mut dry_runtime: DryRunRuntime = DryRunRuntime::new(&mut runtime);
        let mut invoker = Invoker::new(registry, matrix, backend, None);
        invoker.invoke(id, args, &mut dry_runtime, cancel, &mut sink)
    } else {
        let mut invoker = Invoker::new(registry, matrix, backend, None);
        invoker.invoke(id, args, &mut runtime, cancel, &mut sink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `classify` must fold a mid-step cancellation into the Cancelled state.
    #[test]
    fn classify_reports_cancellation_not_failure() {
        let cancelled_runtime_error = Err(rosetta::error::RosettaError::Runtime {
            reason: "macro cancelled by the operator".into(),
        });
        let (state, note) = classify(cancelled_runtime_error, true);
        assert_eq!(state, RunState::Cancelled);
        assert_eq!(note, "cancelled");

        let (state, _) = classify(
            Err(rosetta::error::RosettaError::Runtime {
                reason: "io".into(),
            }),
            false,
        );
        assert_eq!(state, RunState::Failed);
    }

    #[test]
    fn status_tails_from_offset() {
        let runs = RunRegistry::new();
        let record = runs.insert(RunRecord {
            run_id: "r1".into(),
            macro_id: "m".into(),
            dry: true,
            state: RunState::Completed,
            lines: vec![
                RunLogLine {
                    index: 0,
                    level: "info",
                    step: None,
                    message: "a".into(),
                },
                RunLogLine {
                    index: 1,
                    level: "info",
                    step: None,
                    message: "b".into(),
                },
            ],
            result: Some("completed".into()),
            finished_at: Instant::now(),
            cancel: CancellationToken::new(),
            stop_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        });
        let _ = record;
        let status = runs.status("r1", 1).expect("status");
        assert_eq!(status.lines.len(), 1);
        assert_eq!(status.next_offset, 2);
    }
}
