//! Command enqueueing plus the program/console/home facade methods of
//! [`CNCController`].

use super::{CNCController, Command, ControllerSnapshot};
use crate::cnc_api::{CncResult, CoordinateSystem};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// One machine command may carry at most this many bytes of console text.
const MAX_COMMAND_LEN: usize = 4096;
/// How long `execute_command` waits for the worker to reply.
const COMMAND_REPLY_TIMEOUT: Duration = Duration::from_secs(5);

// Some facade methods (device listing, machine-level cancellation, emergency
// stop) are only called from the Tauri binary; the axum binary can leave them
// unused, so the original `#[allow(dead_code)]` marker is kept here.
#[allow(dead_code)]
impl CNCController {
    /// Logs the command description, then hands it to the worker thread.
    ///
    /// A send error means the worker thread is gone (it exits only when both
    /// senders drop); the message is journalled so the UI can surface it.
    pub(super) fn enqueue(&self, command: Command) {
        let description = describe_command(&command);
        let source = if matches!(command, Command::Spindle(_)) {
            "spindle"
        } else {
            "UI command"
        };
        // Severity identifiers are English (info/error/alarm); see message_log.rs.
        crate::message_log::record("info", source, &description);

        if let Err(error) = self.commands.send(command) {
            crate::message_log::record(
                "error",
                "UI command",
                &format!("Command not sent: {error}"),
            );
            eprintln!("[CncApi] worker unavailable: {error}");
        }
    }

    /// Identifier of the device the current adapter reports as connected.
    pub fn connected_device_id(&self) -> Option<String> {
        self.api.as_ref().and_then(|api| api.connected_device_id())
    }

    /// Devices discovered by the current adapter (FluidNC scan results).
    pub fn discovered_devices(&self) -> Vec<cnc_core::discovery::DiscoveredDevice> {
        self.api
            .as_ref()
            .map(|api| api.discovered_devices())
            .unwrap_or_default()
    }

    /// Clone of the underlying adapter, used by the discovery endpoints to
    /// stream live rescans without touching the command queue.
    pub fn discovery_api(&self) -> Option<std::sync::Arc<dyn crate::cnc_api::CncApi>> {
        self.api.clone()
    }

    /// Handle the web layer flips to cancel an in-flight machine command.
    pub fn command_cancellation_handle(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.command_cancel_requested.clone()
    }

    /// Handle the web layer flips to halt current motion immediately.
    pub fn motion_stop_handle(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.motion_stop_requested.clone()
    }

    /// Requests cancellation of the running machine command and motion stop.
    pub fn cancel_machine_command(&self) {
        self.command_cancel_requested.store(true, Ordering::Release);
        self.motion_stop_requested.store(true, Ordering::Release);
    }

    /// Returns the cached snapshot without a network request.
    pub fn get_status_snapshot(&self) -> super::ControllerSnapshot {
        self.status.lock().unwrap().clone()
    }

    /// Queues a program load and waits until the adapter has accepted or rejected it.
    pub fn load_program(&self, path: &Path) -> CncResult<()> {
        let (reply_sender, reply_receiver) = mpsc::channel();
        self.enqueue(Command::LoadProgram(path.to_path_buf(), reply_sender));
        reply_receiver
            .recv_timeout(COMMAND_REPLY_TIMEOUT)
            .map_err(|error| {
                crate::cnc_api::ControllerError::new(
                    "program_load_timeout",
                    format!("No response while loading the program: {error}"),
                )
            })?
    }

    pub fn program_run(&self) -> CncResult<()> {
        self.enqueue(Command::ProgramRun);
        Ok(())
    }

    pub fn start_program(&self) -> CncResult<()> {
        self.program_run()
    }

    pub fn program_pause(&self) -> CncResult<()> {
        self.enqueue(Command::ProgramPause);
        Ok(())
    }

    pub fn program_resume(&self) -> CncResult<()> {
        self.enqueue(Command::ProgramResume);
        Ok(())
    }

    pub fn program_abort(&self) -> CncResult<()> {
        self.enqueue(Command::ProgramAbort);
        Ok(())
    }

    /// Pushes the E-Stop onto the priority channel; the worker checks it
    /// before the regular command queue and drops queued commands.
    pub fn emergency_stop(&self) {
        crate::message_log::record("alarm", "UI command", "Emergency stop");
        if let Err(error) = self.emergency_stop.send(()) {
            eprintln!("[LinuxCncApi] emergency-stop worker unavailable: {error}");
        }
    }

    pub fn reset_alarm(&self) {
        self.enqueue(Command::ResetAlarm);
    }

    pub fn select_coordinate_system(&self, system: CoordinateSystem) {
        self.enqueue(Command::SelectCoordinateSystem(system));
    }

    pub fn set_work_zero(&self, system: CoordinateSystem) {
        self.enqueue(Command::SetWorkZero(system));
    }

    pub fn home_all(&self) {
        self.enqueue(Command::HomeAll);
    }

    pub fn home_axis(&self, axis: crate::cnc_api::Axis) {
        self.enqueue(Command::Home(axis));
    }

    pub fn zero_axis(&self, axis: crate::cnc_api::Axis) {
        self.enqueue(Command::SetAxisWorkZero(axis));
    }

    /// Fire-and-forget MDI line (no reply expected).
    pub fn send_command(&self, command: &str) {
        self.enqueue(Command::Mdi(command.to_string()));
    }

    /// Runs one console command synchronously: the worker executes it and
    /// sends the reply back through the embedded channel.
    pub fn execute_command(&self, command: &str) -> CncResult<String> {
        if command.trim().is_empty()
            || command.len() > MAX_COMMAND_LEN
            || command.contains('\n')
            || command.contains('\r')
        {
            return Err(crate::cnc_api::ControllerError::new(
                "bad_command",
                "Enter one non-empty command without line breaks",
            ));
        }

        self.command_cancel_requested
            .store(false, Ordering::Release);
        let (reply_sender, reply_receiver) = std::sync::mpsc::channel();
        self.enqueue(Command::Execute(command.trim().to_string(), reply_sender));

        reply_receiver
            .recv_timeout(COMMAND_REPLY_TIMEOUT)
            .map_err(|error| {
                crate::cnc_api::ControllerError::new(
                    "command_timeout",
                    format!("No response from the machine command: {error}"),
                )
            })?
    }

    /// Cheap-clonable handle the Rosetta macro host uses to drive blocking
    /// machine commands from its own OS thread (T26 integration, plan §3.1).
    /// The handle carries no locks held across a macro run, so the web
    /// handlers keep responding while a long macro streams its steps.
    pub fn runtime_handle(&self) -> ControllerHandle {
        ControllerHandle {
            commands: self.commands.clone(),
            status: self.status.clone(),
            command_cancel_requested: self.command_cancel_requested.clone(),
        }
    }
}

/// Cloned command/status/cancellation handles of one controller worker pair.
///
/// Mirrors the blocking half of [`CNCController`] (`Execute` + snapshot
/// reads) for callers that must not hold the `AppState` controller mutex for
/// the duration of a macro run. Cancellation is cooperative: the UI "Stop"
/// flips the shared `command_cancel_requested` flag, the adapter in flight
/// aborts the current step and replies with an error, and the runtime checks
/// the flag before every next step.
#[derive(Clone)]
pub struct ControllerHandle {
    commands: mpsc::Sender<Command>,
    status: Arc<Mutex<ControllerSnapshot>>,
    command_cancel_requested: Arc<AtomicBool>,
}

impl ControllerHandle {
    /// Clone of the latest cached status snapshot (no network call).
    pub fn snapshot(&self) -> ControllerSnapshot {
        self.status.lock().unwrap().clone()
    }

    /// Whether the UI requested cancellation of the current machine command.
    pub fn cancel_requested(&self) -> bool {
        self.command_cancel_requested.load(Ordering::Acquire)
    }

    /// Shared cancellation flag itself, so the Rosetta run registry can raise
    /// it when a macro run is cancelled (aborts the blocking step in flight).
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.command_cancel_requested.clone()
    }

    /// Clears the cancellation flag before starting a fresh macro run.
    pub fn reset_cancellation(&self) {
        self.command_cancel_requested
            .store(false, Ordering::Release);
    }

    /// Sends one `Command::Execute` and blocks until the worker replies.
    ///
    /// Deliberately has no reply timeout, unlike the console command path:
    /// a macro step (probing, long feed moves) legitimately runs for
    /// minutes; if the worker drops the command (E-Stop purge) the reply
    /// channel disconnects and this returns an error instead of hanging.
    pub fn execute_blocking(&self, command: &str) -> CncResult<String> {
        if command.trim().is_empty()
            || command.len() > MAX_COMMAND_LEN
            || command.contains('\n')
            || command.contains('\r')
        {
            return Err(crate::cnc_api::ControllerError::new(
                "bad_command",
                "A macro step must be one non-empty command without line breaks",
            ));
        }

        let (reply_sender, reply_receiver) = std::sync::mpsc::channel();
        // Journal the step the same way the controller facade does, so the
        // message log shows macro traffic alongside UI commands.
        crate::message_log::record("info", "rosetta macro", &describe_command_ref(command));
        if let Err(error) = self
            .commands
            .send(Command::Execute(command.trim().to_string(), reply_sender))
        {
            crate::message_log::record(
                "error",
                "rosetta macro",
                &format!("Macro step not sent: {error}"),
            );
            return Err(crate::cnc_api::ControllerError::new(
                "worker_gone",
                format!("Controller worker unavailable: {error}"),
            ));
        }

        reply_receiver.recv().map_err(|error| {
            crate::cnc_api::ControllerError::new(
                "command_cancelled",
                format!("Macro step was not answered: {error}"),
            )
        })?
    }
}

/// Console-command description used by [`ControllerHandle::execute_blocking`]
/// (the `Command::Execute` arm of [`describe_command`] without constructing
/// the full command variant twice).
fn describe_command_ref(command: &str) -> String {
    format!("Rosetta macro step: {command}")
}

/// Human-readable journal description of a queued command.
fn describe_command(command: &Command) -> String {
    use crate::cnc_api::SpindleDirection as ApiSpindleDirection;
    match command {
        Command::LoadProgram(path, _) => format!("LoadProgram {}", path.display()),
        Command::ProgramRun => "ProgramRun".into(),
        Command::ProgramPause => "ProgramPause".into(),
        Command::ProgramResume => "ProgramResume".into(),
        Command::ProgramAbort => "ProgramAbort".into(),
        Command::ResetAlarm => "ResetAlarm".into(),
        Command::SelectCoordinateSystem(system) => {
            format!("SelectCoordinateSystem {}", system.label())
        }
        Command::SetWorkZero(system) => format!("SetWorkZero {}", system.label()),
        Command::SetAxisWorkZero(axis) => format!("SetAxisWorkZero {axis:?}"),
        Command::HomeAll => "HomeAll".into(),
        Command::Home(axis) => format!("Home {axis:?}"),
        Command::Mdi(command) => format!("MDI: {command}"),
        Command::Execute(command, _) => format!("Machine command: {command}"),
        Command::Spindle(command) => match command.direction {
            ApiSpindleDirection::Stopped => {
                format!("Spindle: stop command, target {:.0} RPM", command.rpm)
            }
            ApiSpindleDirection::Clockwise => {
                format!("Spindle: clockwise start, {:.0} RPM", command.rpm)
            }
            ApiSpindleDirection::CounterClockwise => {
                format!("Spindle: counter-clockwise start, {:.0} RPM", command.rpm)
            }
        },
        Command::JogIncrement(request) => format!(
            "Jog: axis {:?}, direction {:+}, distance {:.3}, feed {:.3}",
            request.axis,
            request.direction,
            request.distance,
            request.feed_rate.unwrap_or(0.0),
        ),
        Command::JogContinuous(axis, direction, feed) => {
            if *direction == 0 {
                format!("Jog: stop axis {axis:?}")
            } else {
                format!(
                    "Jog: continuous move of axis {axis:?}, direction {direction:+}, feed {feed:.3}"
                )
            }
        }
        Command::JogContinuousXY(x, y, feed) => {
            if *x == 0 && *y == 0 {
                "Jog: stop XY motion".into()
            } else {
                format!("Jog: continuous XY move, X {x:+}, Y {y:+}, feed {feed:.3}")
            }
        }
        Command::JogCancel => "Jog: cancel motion".into(),
    }
}
