//! The worker loop that owns the adapter: drains the command queue, services
//! the E-Stop channel, honors cancel/motion-stop flags and refreshes the
//! shared status snapshot at [`STATUS_POLL_INTERVAL`] cadence.
//!
//! Runs on the `cnc-api-worker` thread inside a current-thread tokio runtime;
//! every async adapter call is driven with `runtime.block_on` so the facade
//! methods stay synchronous for the UI.

use super::snapshot::{
    error_alert, record_adapter_message, set_ui_alert, STATUS_POLL_INTERVAL,
};
use super::tracking::ProgramTracking;
use super::{CncApi, Command, ControllerSnapshot};
use crate::cnc_api::SpindleDirection as ApiSpindleDirection;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

/// Main worker loop; returns when both command senders are dropped.
pub(super) fn api_worker(
    api: Arc<dyn CncApi>,
    receiver: mpsc::Receiver<Command>,
    emergency_receiver: mpsc::Receiver<()>,
    shared_status: Arc<Mutex<ControllerSnapshot>>,
    command_cancel_requested: Arc<AtomicBool>,
    motion_stop_requested: Arc<AtomicBool>,
    runtime: &tokio::runtime::Runtime,
) {
    let configuration = runtime.block_on(api.machine_configuration()).ok();
    shared_status.lock().unwrap().machine_configuration = configuration;
    let mut last_adapter_message = None;
    let mut spindle_was_active = false;
    let mut reached_spindle_target = None;
    let mut next_status_poll = Instant::now();
    let mut program_tracking = ProgramTracking::default();

    loop {
        if motion_stop_requested.swap(false, Ordering::AcqRel) {
            match runtime.block_on(api.stop_motion()) {
                Ok(()) => {
                    crate::message_log::record(
                        "info",
                        "UI command",
                        "Current machine motion stopped",
                    );
                }
                Err(error) => {
                    crate::message_log::record(
                        "error",
                        "CNC/adapter",
                        &format!(
                            "Failed to stop motion: {}: {}",
                            error.code, error.message
                        ),
                    );
                    set_ui_alert(&shared_status, &error.code, &error.message);
                }
            }
        }

        // E-Stop is checked before the regular command queue.
        match emergency_receiver.try_recv() {
            Ok(()) => {
                // Discard commands queued before the emergency stop.
                while receiver.try_recv().is_ok() {}
                if let Err(error) = runtime.block_on(api.estop()) {
                    eprintln!("[CncApi] E-Stop failed: {error:?}");
                    set_ui_alert(&shared_status, &error.code, &error.message);
                }
                continue;
            }
            Err(mpsc::TryRecvError::Disconnected) => return,
            Err(mpsc::TryRecvError::Empty) => {}
        }

        match receiver.recv_timeout(Duration::from_millis(10)) {
            Ok(command) => {
                let command = match command {
                    Command::Execute(command, reply) => {
                        let result = runtime.block_on(
                            api.execute_command(&command, command_cancel_requested.clone()),
                        );
                        match &result {
                            Ok(output) => {
                                crate::message_log::record(
                                    "info",
                                    "machine command",
                                    &format!("{command} → {output}"),
                                );
                            }
                            Err(error) => {
                                crate::message_log::record(
                                    "error",
                                    "machine command",
                                    &format!("{command}: {}: {}", error.code, error.message),
                                );
                                set_ui_alert(&shared_status, &error.code, &error.message);
                            }
                        }
                        let _ = reply.send(result);
                        continue;
                    }
                    command => command,
                };

                // E-Stop may have arrived while the worker was waiting for a regular command.
                if emergency_receiver.try_recv().is_ok() {
                    while receiver.try_recv().is_ok() {}
                    if let Err(error) = runtime.block_on(api.estop()) {
                        eprintln!("[CncApi] E-Stop failed: {error:?}");
                        set_ui_alert(&shared_status, &error.code, &error.message);
                    }
                    continue;
                }
                // Remember which command starts or stops the program before `command` is moved.
                let starts_program = matches!(command, Command::ProgramRun);
                let stops_program =
                    matches!(command, Command::ProgramAbort | Command::LoadProgram(_));
                let result =
                    runtime.block_on(dispatch_command(&api, &shared_status, command));

                match result {
                    Ok(()) => {
                        shared_status.lock().unwrap().ui_alert = None;
                        if starts_program {
                            program_tracking.started();
                        }
                        if stops_program {
                            program_tracking.stopped();
                        }
                    }
                    Err(error) => {
                        eprintln!("[CncApi] {}: {}", error.code, error.message);
                        crate::message_log::record(
                            "error",
                            "CNC/adapter",
                            &format!("{}: {}", error.code, error.message),
                        );
                        set_ui_alert(&shared_status, &error.code, &error.message);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }

        poll_status(
            &api,
            runtime,
            &shared_status,
            &mut last_adapter_message,
            &mut program_tracking,
            &mut next_status_poll,
            &mut spindle_was_active,
            &mut reached_spindle_target,
        );
    }
}

/// Routes one queued command to the adapter. The `Execute` variant is fully
/// handled by the loop before dispatch, hence the invariant branch. The
/// shared status is updated here only for the toolpath bookkeeping that must
/// happen atomically around `LoadProgram`.
async fn dispatch_command(
    api: &Arc<dyn CncApi>,
    shared_status: &Arc<Mutex<ControllerSnapshot>>,
    command: Command,
) -> crate::cnc_api::CncResult<()> {
    match command {
        Command::LoadProgram(path) => {
            // Drop the previous toolpath first so a failed load never shows stale geometry.
            shared_status.lock().unwrap().toolpath = cnc_core::Toolpath::default();
            let result = api.load_program(&path).await;
            if result.is_ok() {
                let toolpath = load_toolpath_from_file(&path);
                shared_status.lock().unwrap().toolpath = toolpath;
            }
            result
        }
        Command::ProgramRun => api.program_run(0).await,
        Command::ProgramPause => api.program_pause().await,
        Command::ProgramResume => api.program_resume().await,
        Command::ProgramAbort => api.program_abort().await,
        Command::ResetAlarm => match api.clear_error().await {
            Ok(()) => api.power_on().await,
            Err(error) => Err(error),
        },
        Command::SelectCoordinateSystem(system) => api.select_coordinate_system(system).await,
        Command::SetWorkZero(system) => api.set_work_zero(system).await,
        Command::SetAxisWorkZero(axis) => api.set_axis_work_zero(axis).await,
        Command::HomeAll => api.home(None).await,
        Command::Home(axis) => api.home(Some(axis)).await,
        Command::Mdi(command) => api.mdi(&command).await,
        Command::Execute(_, reply) => {
            let error = crate::cnc_api::ControllerError::new(
                "internal",
                "command dispatch invariant violated",
            );
            let _ = reply.send(Err(error.clone()));
            Err(error)
        }
        Command::Spindle(command) => {
            eprintln!(
                "[Backend/Worker] sending spindle command to API: direction={:?}, rpm={:.0}",
                command.direction, command.rpm
            );
            let result = api.set_spindle(command).await;
            match &result {
                Ok(()) => eprintln!("[Backend/Worker] spindle command accepted by API"),
                Err(error) => eprintln!(
                    "[Backend/Worker] spindle command failed: {}: {}",
                    error.code, error.message
                ),
            }
            result
        }
        Command::JogIncrement(request) => api.jog_increment(request).await,
        Command::JogContinuous(axis, direction, feed_rate) => {
            api.jog_continuous(axis, direction, feed_rate).await
        }
        Command::JogContinuousXY(x_direction, y_direction, feed_rate) => {
            api.jog_continuous_xy(x_direction, y_direction, feed_rate).await
        }
        Command::JogCancel => api.cancel_jog().await,
    }
}

/// Reads a G-code file and parses it into the universal toolpath.
///
/// Returns an empty toolpath when the file cannot be read; the adapter has
/// already accepted the program, so the UI simply shows no traversed geometry.
fn load_toolpath_from_file(path: &Path) -> cnc_core::Toolpath {
    match std::fs::read_to_string(path) {
        Ok(source) => cnc_core::parse_toolpath(&source),
        Err(error) => {
            crate::message_log::record(
                "error",
                "toolpath",
                &format!("Failed to read program for toolpath: {error}"),
            );
            cnc_core::Toolpath::default()
        }
    }
}

/// One status refresh pass: message journaling, program-progress gating,
/// spindle events, reconnect configuration reload and error marking.
#[allow(clippy::too_many_arguments)]
fn poll_status(
    api: &Arc<dyn CncApi>,
    runtime: &tokio::runtime::Runtime,
    shared_status: &Arc<Mutex<ControllerSnapshot>>,
    last_adapter_message: &mut Option<String>,
    program_tracking: &mut ProgramTracking,
    next_status_poll: &mut Instant,
    spindle_was_active: &mut bool,
    reached_spindle_target: &mut Option<(ApiSpindleDirection, u64)>,
) {
    // Status polling at the worker's 10 ms command cadence floods FluidNC's
    // WebSocket with realtime '?' queries. Limit polling to 10 Hz instead.
    let now = Instant::now();
    if now < *next_status_poll {
        return;
    }
    *next_status_poll = now + STATUS_POLL_INTERVAL;

    match runtime.block_on(api.status()) {
        Ok(mut status) => {
            if let Some(message) = status.message.as_deref() {
                let severity = if status.task_state == crate::cnc_api::TaskState::Estop
                    || message.to_ascii_lowercase().starts_with("alarm")
                {
                    "alarm"
                } else if message.to_ascii_lowercase().starts_with("error") {
                    "error"
                } else {
                    "info"
                };

                // Regular status replies (for example "FluidNC Idle") arrive on every
                // poll and are not journal events, so only alarm/error are recorded.
                if severity != "info" {
                    record_adapter_message(last_adapter_message, severity, message);
                }
            } else {
                *last_adapter_message = None;
            }

            // Progress is reported only while the controller runs the loaded program.
            program_tracking.observe(status.task_state, status.interpreter_state);
            if !program_tracking.is_active() {
                status.progress = cnc_core::ToolpathProgress::unavailable();
            }

            let spindle = &status.spindle;
            let spindle_active =
                spindle.enabled || spindle.actual_rpm > 0.01 || spindle.commanded_rpm > 0.01;
            let tolerance = (spindle.commanded_rpm.abs() * 0.01).max(1.0);
            let at_target = spindle.direction != ApiSpindleDirection::Stopped
                && spindle.commanded_rpm > 0.0
                && (spindle.actual_rpm - spindle.commanded_rpm).abs() <= tolerance;
            let target_key = (spindle.direction, spindle.commanded_rpm.to_bits());

            if at_target && *reached_spindle_target != Some(target_key) {
                crate::message_log::record(
                    "info",
                    "spindle",
                    &format!(
                        "Spindle reached target speed: {:.0} RPM",
                        spindle.commanded_rpm
                    ),
                );
                *reached_spindle_target = Some(target_key);
            } else if !at_target {
                *reached_spindle_target = None;
            }

            let spindle_stopped =
                !spindle.enabled && spindle.actual_rpm <= 0.01 && spindle.commanded_rpm <= 0.01;
            if *spindle_was_active && spindle_stopped {
                crate::message_log::record("info", "spindle", "Spindle stopped: 0 RPM");
                *reached_spindle_target = None;
            }
            *spindle_was_active = spindle_active && !spindle_stopped;

            let reconnected = !shared_status.lock().unwrap().machine.connected;
            if reconnected {
                if let Ok(configuration) = runtime.block_on(api.machine_configuration()) {
                    shared_status.lock().unwrap().machine_configuration = Some(configuration);
                }
            }

            let mut current = shared_status.lock().unwrap();
            current.machine = status;
            if reconnected {
                current.machine_configuration =
                    runtime.block_on(api.machine_configuration()).ok();
            }
            if current
                .ui_alert
                .as_ref()
                .is_some_and(|(kind, _)| kind == "connection")
            {
                current.ui_alert = None;
            }
        }
        Err(error) => {
            eprintln!("[CncApi] status {}: {}", error.code, error.message);
            record_adapter_message(
                last_adapter_message,
                "error",
                &format!("status {}: {}", error.code, error.message),
            );
            let mut current = shared_status.lock().unwrap();
            current.machine.connected = false;
            current.machine.message = Some(error.message.clone());
            current.ui_alert = Some(error_alert("connection", &error.message));
        }
    }
}
