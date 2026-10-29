//! Queues UI commands on a worker thread and maintains a cached controller status.
//!
//! The implementation used to live in a single 1100-line file; it is now a
//! directory module split by responsibility:
//!
//! - `snapshot` — `ControllerSnapshot` / `SpindleDirection` types and the
//!   alert helpers shared with the worker thread.
//! - `connect`  — the two constructors that spawn the API worker thread
//!   (legacy socket path and generic `Arc<dyn CncApi>`).
//! - `queue`    — command enqueueing plus the program/console/home facade.
//! - `jog`      — jog-speed settings and all step/continuous jog facade methods.
//! - `spindle`  — spindle setpoint and direction facade methods.
//! - `tracking` — `ProgramTracking`: decides when toolpath progress is real.
//! - `worker`   — the actual worker loop (`api_worker`) executed on its thread.
//!
//! Every submodule is private; the names external consumers use
//! (`CNCController`, `ControllerSnapshot`, `SpindleDirection`) stay reachable
//! through this module exactly as before the split. Private items declared
//! here (`Command`, the struct fields, `enqueue`) remain visible to the child
//! modules, which is what lets the impl blocks be spread across several files.

mod connect;
mod jog;
mod queue;
mod snapshot;
mod spindle;
mod tracking;
mod worker;

pub use snapshot::{ControllerSnapshot, SpindleDirection};

use crate::cnc_api::{Axis, CncApi, CoordinateSystem, SpindleCommand};
use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, mpsc, Arc, Mutex};

/// One unit of work handed from the UI thread to the API worker thread.
///
/// `Execute` carries a reply channel so `CNCController::execute_command` can
/// block until the machine answers; everything else is fire-and-forget.
#[derive(Debug)]
enum Command {
    LoadProgram(PathBuf),
    ProgramRun,
    ProgramPause,
    ProgramResume,
    ProgramAbort,
    ResetAlarm,
    SelectCoordinateSystem(CoordinateSystem),
    SetWorkZero(CoordinateSystem),
    SetAxisWorkZero(Axis),
    HomeAll,
    Home(Axis),
    Mdi(String),
    Execute(String, mpsc::Sender<crate::cnc_api::CncResult<String>>),
    Spindle(SpindleCommand),
    JogIncrement(crate::cnc_api::JogRequest),
    JogContinuous(Axis, i8, f64),
    JogContinuousXY(i8, i8, f64),
    JogCancel,
}

/// UI-facing facade around an adapter running on a dedicated worker thread.
///
/// All fields are private but stay visible to the child modules of this file,
/// where the individual impl blocks live.
pub struct CNCController {
    /// Shared status snapshot polled by the worker and read by the UI handlers.
    pub status: Arc<Mutex<ControllerSnapshot>>,
    /// Concrete adapter when the controller was built from one; `None` for
    /// the legacy socket constructor that connects lazily on the worker.
    api: Option<Arc<dyn CncApi>>,
    commands: mpsc::Sender<Command>,
    emergency_stop: mpsc::Sender<()>,
    xy_jog_speed: Mutex<f64>,
    z_jog_speed: Mutex<f64>,
    /// Shared jog feed for the rotary axes A, B and C (mm/min or deg/min, depending on the axis).
    rotary_jog_speed: Mutex<f64>,
    spindle_speed_setpoint: Mutex<f64>,
    /// Set by `cancel_machine_command`; checked by adapters during long runs.
    command_cancel_requested: Arc<AtomicBool>,
    /// Set when the UI requests an immediate motion halt.
    motion_stop_requested: Arc<AtomicBool>,
}
