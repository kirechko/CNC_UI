//! Asynchronous GRBL-over-serial adapter (`CncApi` implementation).
//!
//! The file used to be a single 2100-line module; it is now split into
//! focused child modules that all share the [`GrblAdapter`] type and its
//! [`Session`] state. Rust visibility makes this ergonomic: an item declared
//! private in this file is visible to every child module below it, while the
//! inherent helper methods that children call across files are marked
//! `pub(super)` so they stay reachable inside this module subtree without
//! becoming part of the public crate API.
//!
//! Module map:
//! * `connect`  - opening the serial port and building the initial session.
//! * `serial`   - line-level transport: write/read commands, ACK protocol.
//! * `query`    - realtime `?` status queries and `$$` settings queries.
//! * `status`   - parsing the `<...|...>` status report into `MachineStatus`.
//! * `guards`   - argument validation and motion-state guard helpers.
//! * `jog`      - jog command formatting and chunked continuous jog.
//! * `program`  - stop-and-wait program streaming helpers and file loading.
//! * `spindle`  - spindle command normalization and limits.
//! * `api`      - the `CncApi` trait implementation facade.
//! * `tests`    - unit tests for commands, guards and the transport layer.
//! * `status_tests` - unit tests for the realtime status-report parser.

use crate::{toolpath::Toolpath, Axis, CoordinateSystem, SpindleCommand, ToolpathProgress};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Mutex;
use tokio_serial::SerialStream;

mod api;
mod connect;
mod guards;
mod jog;
mod program;
mod query;
mod serial;
mod spindle;
mod status;
#[cfg(test)]
mod status_tests;
#[cfg(test)]
mod tests;

/// How long a regular command waits for its `ok`/`error` acknowledgement.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(3);
/// Extended ACK budget for streamed program lines. GRBL only acknowledges a
/// line once there is room in the planner buffer again, so a full buffer in
/// front of a long feed move can delay the `ok` of the pending line far past
/// the regular 3-second window. Treating that as a protocol failure would
/// poison the session while the machine is simply executing a slow block.
const PROGRAM_ACK_TIMEOUT: Duration = Duration::from_secs(45);
/// How long the realtime `?` status request waits for a report line.
const STATUS_TIMEOUT: Duration = Duration::from_secs(1);
/// Overall budget for the startup handshake that synchronizes with GRBL after
/// the serial port open (which toggles DTR/RTS and resets Arduino boards).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(3500);
/// How long the handshake waits for bytes between two realtime `?` pings.
const HANDSHAKE_PING_INTERVAL: Duration = Duration::from_millis(500);
/// Quiet period after which the boot-noise drain considers the port idle.
const HANDSHAKE_DRAIN_GAP: Duration = Duration::from_millis(120);
/// Largest accepted length of a single GRBL response line, in bytes.
const MAX_RESPONSE_LINE: usize = 4096;
/// GRBL accepts at most this many bytes per command line.
const MAX_COMMAND_LENGTH: usize = 80;
/// Programs larger than this are rejected before streaming (GRBL buffer).
const MAX_PROGRAM_BYTES: usize = 5 * 1024 * 1024;
/// Distance covered by one chunk of a continuous jog move, in millimeters.
const JOG_CHUNK_MM: f64 = 10.0;

/// Pending continuous-jog parameters replayed chunk by chunk while Idle.
#[derive(Clone, Copy)]
struct ContinuousJog {
    x_direction: i8,
    y_direction: i8,
    axis: Option<Axis>,
    feed_rate: f64,
}

/// Mutable serial-session state kept behind the adapter mutex.
struct Session {
    port: SerialStream,
    rx_buffer: Vec<u8>,
    /// True until the startup handshake (soft reset + banner/status sync) has
    /// completed once. Opening the port toggles DTR/RTS, which resets most
    /// Arduino-based GRBL boards and makes them emit a boot banner; deferring
    /// the handshake to the first status query keeps `connect()` synchronous
    /// and non-blocking for the runtime that hosts the adapter.
    handshake_pending: bool,
    work_offsets: HashMap<CoordinateSystem, [f64; 3]>,
    protocol_poisoned: bool,
    program_streaming: bool,
    jog_chunk_active: bool,
    program: Vec<String>,
    /// Parsed toolpath of the loaded program, used to report progress.
    toolpath: Toolpath,
    /// Last forward-only toolpath progress reported while streaming. GRBL's
    /// stop-and-wait acknowledgements let the planner run ahead of the
    /// physical tool, so the traversed distance may only ever grow: keeping
    /// the previous value lets the adapter freeze (not rewind) the highlight
    /// when a position sample momentarily misses the path window. Mirrors the
    /// FluidNC WebSocket adapter (`Session::last_progress`).
    last_progress: Option<ToolpathProgress>,
    program_name: Option<String>,
    current_line: u32,
    /// Modal motion group (G0/G1/G2/G3) of the last motion word the adapter
    /// sent, used for the telemetry mode indication (task T12): GRBL's '?'
    /// report does not expose it, so the adapter remembers what it streamed.
    motion_mode: Option<String>,
    active_coordinate_system: CoordinateSystem,
    spindle_command: SpindleCommand,
    min_spindle_rpm: Option<f64>,
    max_spindle_rpm: Option<f64>,
    continuous_jog: Option<ContinuousJog>,
}

/// GRBL controller reached through an async serial port.
pub struct GrblAdapter {
    // The Tokio mutex serializes complete GRBL transactions, including I/O waits,
    // without blocking the runtime thread during await.
    session: Mutex<Session>,
    serial_endpoint: String,
}

/// Polls the cancellation flag until the UI requests a command abort.
/// Used with `tokio::select!` so long console queries stay cancellable.
async fn wait_for_command_cancellation(cancellation: Arc<AtomicBool>) {
    loop {
        if cancellation.load(Ordering::Acquire) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
