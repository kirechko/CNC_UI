//! `HostRuntime` implementation over the application's `CNCController`
//! (T26 plan §3.1 — "глазная игла #1").
//!
//! The Rosetta macro engine is synchronous; this runtime adapts it onto the
//! controller's blocking `Execute` command path (`Command::Execute` carries a
//! reply channel, see `src/cnc_controller/queue.rs`). A macro run lives on its
//! own OS thread and drives the machine exclusively through [`ControllerHandle`]
//! clones, so it never holds the `AppState` controller mutex and the web
//! handlers keep polling status while the macro streams its steps.
//!
//! Cancellation is cooperative and two-layered:
//! * the engine's [`CancellationToken`] is checked by the interpreter between
//!   statements;
//! * [`ControllerRuntime::send_command`] additionally polls the controller's
//!   shared `command_cancel_requested` flag before each step and while
//!   sleeping, so the existing UI "Stop" button (`POST /api/command/cancel`)
//!   also aborts a running macro.

use std::thread::sleep;
use std::time::Duration;

use rosetta::error::{RosettaError, RosettaResult};
use rosetta::ir::macro_ir::Value;
use rosetta::macro_engine::runtime::{HostRuntime, MachineState};

use crate::cnc_controller::ControllerHandle;

/// Maximum single `sleep` slice inside [`HostRuntime::sleep_ms`]; the loop
/// re-checks the cancellation flag after every slice so long `delay` steps
/// stay responsive to the UI Stop button.
const CANCEL_POLL_SLICE: Duration = Duration::from_millis(100);

/// Retry delay while the controller reports `motion_busy` (the emulator
/// rejects a new motion until the previous move completed; the reactive
/// interpreter's stop-and-wait semantics therefore boil down to "re-send the
/// step when the machine is free again", exactly like GRBL's `$J` streaming
/// handshake on the host).
const BUSY_RETRY_DELAY: Duration = Duration::from_millis(100);

/// Upper bound for waiting out one `motion_busy` conflict; a move longer than
/// this is a controller fault, not a macro pacing problem.
const BUSY_RETRY_TIMEOUT: Duration = Duration::from_secs(120);

/// `HostRuntime` over a cloned [`ControllerHandle`].
pub struct ControllerRuntime {
    /// Blocking command + cached-status handle of the active worker pair.
    handle: ControllerHandle,
}

impl ControllerRuntime {
    /// Wrap a controller handle for one macro run thread.
    pub fn new(handle: ControllerHandle) -> Self {
        ControllerRuntime { handle }
    }

    /// Machine-state mapping shared with the run-status endpoint (§3.1 table):
    /// `TaskState`/`InterpreterState` → the engine's coarse `MachineState`.
    pub fn machine_state_of(snapshot: &crate::cnc_controller::ControllerSnapshot) -> MachineState {
        use cnc_core::{InterpreterState, TaskMode, TaskState};
        let machine = &snapshot.machine;
        if !machine.connected {
            return MachineState::Disconnected;
        }
        match machine.task_state {
            TaskState::Estop => MachineState::EStop,
            // EstopReset / Off: the controller needs the operator to clear and
            // power on first — the engine must refuse to start (ТЗ №2 §9.2).
            TaskState::EstopReset | TaskState::Off => MachineState::Alarm,
            TaskState::On => match machine.interpreter_state {
                InterpreterState::Reading if matches!(machine.task_mode, TaskMode::Auto) => {
                    MachineState::Running
                }
                InterpreterState::Paused => MachineState::Paused,
                _ => MachineState::Idle,
            },
        }
    }

    /// Reads the current coarse state from the cached snapshot.
    fn state(&mut self) -> MachineState {
        Self::machine_state_of(&self.handle.snapshot())
    }

    /// Blocks until the machine reports no active motion (the emulator
    /// clears `motion_mode` when a queued move completes and never reports
    /// homing while idle). This mirrors real MDI-blocking controllers: a
    /// macro must not queue its next step while the previous one travels.
    ///
    /// Returns after the first snapshot showing the machine free; cancellation
    /// aborts the wait with an error so a held-down Stop acts immediately.
    fn wait_until_free(&mut self) -> RosettaResult<()> {
        let deadline = std::time::Instant::now() + BUSY_RETRY_TIMEOUT;
        loop {
            let snapshot = self.handle.snapshot();
            let busy = snapshot.machine.homing || snapshot.machine.motion_mode.is_some();
            if !busy {
                return Ok(());
            }
            if self.handle.cancel_requested() {
                return Err(RosettaError::Runtime {
                    reason: "macro cancelled by the operator".into(),
                });
            }
            if std::time::Instant::now() >= deadline {
                return Err(RosettaError::Runtime {
                    reason: "timed out waiting for the machine to become idle".into(),
                });
            }
            sleep(BUSY_RETRY_DELAY);
        }
    }
}

impl HostRuntime for ControllerRuntime {
    /// Blocks until the worker executed the command on the machine.
    ///
    /// Refuses to start a new step once the UI flipped the cancellation flag;
    /// the error variant (`Runtime`) makes the interpreter unwind the macro
    /// and report the run as failed, which the run registry then shows as
    /// "cancelled" because the flag was set.
    fn send_command(&mut self, command: &str) -> RosettaResult<String> {
        if self.handle.cancel_requested() {
            return Err(RosettaError::Runtime {
                reason: "macro cancelled by the operator".into(),
            });
        }
        // Reactive stop-and-wait semantics (ТЗ №2 §8: `send_command` blocks
        // until the controller *executed* the command): the emulator answers
        // motion lines as soon as they are queued, so the runtime first waits
        // for the machine to free up, then submits. `motion_busy` re-sends
        // cover the residual race between the cached snapshot and the worker.
        self.wait_until_free()?;
        let deadline = std::time::Instant::now() + BUSY_RETRY_TIMEOUT;
        loop {
            let error = match self.handle.execute_blocking(command) {
                Ok(output) => return Ok(output),
                Err(error) => error,
            };
            if error.code != "motion_busy" || std::time::Instant::now() >= deadline {
                return Err(RosettaError::Runtime {
                    reason: format!("{}: {}", error.code, error.message),
                });
            }
            sleep(BUSY_RETRY_DELAY);
        }
    }

    /// Status vocabulary of the application adapter (fields not present in
    /// `MachineStatus` are refused with an explicit error so macro typos and
    /// unsupported probes fail fast, per ТЗ №1 principle 4):
    ///
    /// | key               | value                                        |
    /// |-------------------|----------------------------------------------|
    /// | `pos.x`…`pos.c`   | machine coordinates                          |
    /// | `work.x`…`work.c` | machine coordinates minus the work offset    |
    /// | `feed`, `jog`     | programmed / jog feed rate                   |
    /// | `spindle`         | actual spindle RPM                           |
    /// | `state`           | coarse engine state label                    |
    /// | `idle`            | 1 when the interpreter is idle               |
    /// | `connected`       | 1 while the adapter link is up               |
    /// | `homed.x`…        | per-axis homing flag                         |
    ///
    /// The app adapter does not report probe state (`MachineStatus` has no
    /// probe field); `probe_trig` therefore errors instead of pretending.
    fn status_field(&mut self, key: &str) -> RosettaResult<Value> {
        let snapshot = self.handle.snapshot();
        let machine = &snapshot.machine;
        let num = |value: f64| Value::Num(value);
        // Rotaries and linear axes share one 6-slot indexing helper.
        let axis_pick =
            |pick: &dyn Fn(&cnc_core::Position) -> f64| -> Value { num(pick(&machine.position)) };
        match key {
            "pos.x" => Ok(axis_pick(&|p| p.x)),
            "pos.y" => Ok(axis_pick(&|p| p.y)),
            "pos.z" => Ok(axis_pick(&|p| p.z)),
            "pos.a" => Ok(axis_pick(&|p| p.a)),
            "pos.b" => Ok(axis_pick(&|p| p.b)),
            "pos.c" => Ok(axis_pick(&|p| p.c)),
            "work.x" => Ok(num(machine.position.x - machine.work_offset.x)),
            "work.y" => Ok(num(machine.position.y - machine.work_offset.y)),
            "work.z" => Ok(num(machine.position.z - machine.work_offset.z)),
            "work.a" => Ok(num(machine.position.a - machine.work_offset.a)),
            "work.b" => Ok(num(machine.position.b - machine.work_offset.b)),
            "work.c" => Ok(num(machine.position.c - machine.work_offset.c)),
            "feed" => Ok(num(machine.feed_rate)),
            "jog" => Ok(num(machine.jog_rate)),
            "spindle" => Ok(num(machine.spindle.actual_rpm)),
            "line" => Ok(num(machine.current_line as f64)),
            "state" => Ok(Value::Str(
                match self.state() {
                    MachineState::Idle => "idle",
                    MachineState::Running => "running",
                    MachineState::Paused => "paused",
                    MachineState::Alarm => "alarm",
                    MachineState::EStop => "estop",
                    MachineState::Disconnected => "disconnected",
                    MachineState::Other => "other",
                }
                .to_owned(),
            )),
            "idle" => Ok(num(if matches!(self.state(), MachineState::Idle) {
                1.0
            } else {
                0.0
            })),
            "connected" => Ok(num(if machine.connected { 1.0 } else { 0.0 })),
            "homed.x" => Ok(num(machine.homed[0] as i32 as f64)),
            "homed.y" => Ok(num(machine.homed[1] as i32 as f64)),
            "homed.z" => Ok(num(machine.homed[2] as i32 as f64)),
            "homed.a" => Ok(num(machine.homed[3] as i32 as f64)),
            "homed.b" => Ok(num(machine.homed[4] as i32 as f64)),
            "homed.c" => Ok(num(machine.homed[5] as i32 as f64)),
            other => Err(RosettaError::Runtime {
                reason: format!("status field \"{other}\" is not reported by this backend"),
            }),
        }
    }

    /// Real wall-clock sleep in 100 ms slices, each re-checking the shared
    /// cancellation flag so `delay` steps stay interruptible.
    fn sleep_ms(&mut self, ms: u64) {
        let mut remaining = ms;
        while remaining > 0 {
            if self.handle.cancel_requested() {
                return; // the next send_command reports the cancellation
            }
            let slice = remaining.min(CANCEL_POLL_SLICE.as_millis() as u64);
            sleep(Duration::from_millis(slice));
            remaining -= slice;
        }
    }

    fn machine_state(&mut self) -> MachineState {
        self.state()
    }
}

/// Unit tests of the state mapping — the safety gate depends on it, so the
/// table is pinned here instead of only exercised through live macro runs.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cnc_controller::ControllerSnapshot;
    use cnc_core::{
        CoordinateSystem, InterpreterState, MachineStatus, Position, SpindleDirection,
        SpindleStatus, TaskMode, TaskState, ToolpathProgress,
    };

    /// Builds a snapshot with the state-relevant fields set and the rest
    /// neutral; constructed field-by-field (MachineStatus has no Default).
    fn snapshot(
        connected: bool,
        task_state: TaskState,
        interpreter: InterpreterState,
    ) -> ControllerSnapshot {
        let zero = Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            a: 0.0,
            b: 0.0,
            c: 0.0,
        };
        ControllerSnapshot {
            machine: MachineStatus {
                connected,
                task_mode: TaskMode::Manual,
                task_state,
                interpreter_state: interpreter,
                position: zero,
                work_offset: zero,
                active_coordinate_system: CoordinateSystem::G54,
                coordinate_systems: Vec::new(),
                feed_rate: 0.0,
                spindle: SpindleStatus {
                    commanded_rpm: 0.0,
                    actual_rpm: 0.0,
                    direction: SpindleDirection::Stopped,
                    enabled: false,
                },
                probe_triggered: false,
                probe_position: None,
                selected_tool: None,
                current_tool: None,
                mist_coolant_on: false,
                flood_coolant_on: false,
                jog_rate: 0.0,
                motion_mode: None,
                max_spindle_rpm: 0.0,
                loaded_program: None,
                current_line: 0,
                total_lines: 0,
                message: None,
                homed: [false; 6],
                homing: false,
                progress: ToolpathProgress::default(),
            },
            machine_configuration: None,
            jog_step: 1.0,
            ui_alert: None,
            toolpath: cnc_core::Toolpath::default(),
        }
    }

    #[test]
    fn disconnected_beats_everything() {
        assert_eq!(
            ControllerRuntime::machine_state_of(&snapshot(
                false,
                TaskState::On,
                InterpreterState::Idle
            )),
            MachineState::Disconnected
        );
    }

    #[test]
    fn estop_and_unpowered_map_to_abort_states() {
        assert_eq!(
            ControllerRuntime::machine_state_of(&snapshot(
                true,
                TaskState::Estop,
                InterpreterState::Idle
            )),
            MachineState::EStop
        );
        assert_eq!(
            ControllerRuntime::machine_state_of(&snapshot(
                true,
                TaskState::Off,
                InterpreterState::Idle
            )),
            MachineState::Alarm
        );
    }

    #[test]
    fn powered_idle_is_idle() {
        assert_eq!(
            ControllerRuntime::machine_state_of(&snapshot(
                true,
                TaskState::On,
                InterpreterState::Idle
            )),
            MachineState::Idle
        );
    }
}
