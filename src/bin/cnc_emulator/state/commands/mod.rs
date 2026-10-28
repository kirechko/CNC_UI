//! Implements the emulator's API commands and request dispatch.
//!
//! This module is the request router for the emulator's wire protocol: the
//! [`handle`] entry point maps every supported method name to the concrete
//! command implementation and returns either a JSON payload or a protocol
//! error. Command functions are grouped into child modules by concern:
//!
//! * [`power`] - machine power state, alarm reset, and spindle setpoint;
//! * [`program`] - MDI execution and program load/run/pause/resume/abort;
//! * [`jog`] - incremental, continuous (single axis and XY), cancel/stop;
//! * [`axes`] - coordinate system selection, work-zero setters, and homing.
//!
//! Cross-module visibility: every command function is `pub(super)` so it is
//! reachable from the dispatcher in this `mod.rs` (and anywhere else inside
//! the `state` subtree). The shared [`Emulator`] type and the helper
//! `axis_index` live in the parent `state` module; children reach them via
//! `super::super::...` or the equivalent crate-level paths.
mod axes;
mod jog;
mod power;
mod program;

use super::super::protocol::{api_error, Request, WireError};
use super::Emulator;
use crate::cnc_api::*;
use serde_json::Value;

// Bring the command implementations into this module's scope so the
// dispatcher body below keeps calling them by their bare names.
use axes::{home, select_coordinate_system, set_axis_work_zero, set_work_zero};
use jog::{jog_cancel, jog_continuous, jog_continuous_xy, jog_increment, stop_motion};
use power::{clear_error, power_off, power_on, set_spindle};
use program::{load_program, mdi, program_abort, program_pause, program_resume, program_run};

/// Route one decoded wire request to the matching command implementation.
///
/// The request's `args` are handed to the command as an opaque JSON value;
/// each command validates and deserializes the fields it needs. Only the two
/// read-only queries (`status`, `machine_configuration`) are answered here
/// because they never mutate emulator state.
pub(super) fn handle(emulator: &mut Emulator, request: Request) -> Result<Value, WireError> {
    let args = request.args;
    match request.method.as_str() {
        "status" => Ok(serde_json::to_value(&emulator.status)
            .map_err(|error| api_error("serialization", error.to_string()))?),
        "machine_configuration" => Ok(serde_json::to_value(
            emulator.config.machine_configuration(),
        )
        .map_err(|error| api_error("serialization", error.to_string()))?),
        "power_on" => power_on(emulator),
        "power_off" => power_off(emulator, false),
        "estop" => power_off(emulator, true),
        "select_coordinate_system" => select_coordinate_system(emulator, args),
        "set_work_zero" => set_work_zero(emulator, args),
        "set_axis_work_zero" => set_axis_work_zero(emulator, args),
        "home" => home(emulator, args),
        "jog_increment" => jog_increment(emulator, args),
        "jog_continuous" => jog_continuous(emulator, args),
        "jog_continuous_xy" => jog_continuous_xy(emulator, args),
        "jog_cancel" => jog_cancel(emulator),
        "stop_motion" => stop_motion(emulator),
        "mdi" => mdi(emulator, args),
        "load_program" => load_program(emulator, args),
        "program_run" => program_run(emulator, args),
        "program_pause" => program_pause(emulator),
        "program_resume" => program_resume(emulator),
        "program_abort" => program_abort(emulator),
        "set_spindle" => set_spindle(emulator, args),
        "clear_error" => clear_error(emulator),
        _ => Err(api_error(
            "unknown_method",
            format!("unsupported method: {}", request.method),
        )),
    }
}

impl Emulator {
    /// Guard used by every motion-producing command: the virtual machine must
    /// be powered on (`TaskState::On`) before any movement is accepted.
    pub(super) fn ensure_motion_allowed(&self) -> Result<(), WireError> {
        if self.status.task_state != TaskState::On {
            return Err(api_error(
                "machine_off",
                "virtual machine is not powered on",
            ));
        }
        Ok(())
    }
}
