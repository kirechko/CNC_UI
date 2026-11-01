//! Asynchronous GRBL-over-serial adapter (`CncApi` implementation).
//!
//! Although named after GRBL, the driver serves every controller that speaks
//! the GRBL line protocol over a UART — including **FluidNC on a COM port**
//! (task): select the firmware family with [`SerialProtocol`] when opening
//! the port. FluidNC firmware on ESP32-class boards exposes the same
//! `ok`/`error:`/realtime-byte protocol and even mirrors the `<Idle|MPos:…>`
//! status grammar, so the differences reduce to the discovery id prefix, the
//! command line length, 6-axis position triplets and the accepted axis set.
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
    time::{Duration, Instant},
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

/// Reply state of one program line written to GRBL.
///
/// GRBL acknowledges a motion line only when room appears in the planner
/// buffer, which can take as long as one buffered block. The scheduler
/// therefore keeps *at most one* unacknowledged line (stop-and-wait, the same
/// serial-safety invariant as before) but resolves its reply on the realtime
/// status polls instead of blocking the poll loop: `query_status` routes any
/// `ok` / `error:` / `ALARM:` line it drains into [`PendingProgramAck::route`]
/// and `pump_program_stream` acts on the result at the next scheduler pass.
/// This is what keeps the trajectory/coordinate display smooth: the UI used
/// to freeze for as long as one slow block and then jump by a whole buffered
/// section, because the status call itself waited up to 45 s for the `ok`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct PendingProgramAck {
    /// The exact line GRBL must acknowledge; kept for the tolerated-accessory
    /// decision (`M7/M8/M9` rejections are skipped, not fatal).
    pub(super) line: String,
    /// When the line was written; drives the [`PROGRAM_ACK_TIMEOUT`] budget.
    pub(super) sent_at: Instant,
    /// Set once the reply for `line` has been observed on the wire.
    pub(super) reply: Option<StreamReply>,
}

impl PendingProgramAck {
    /// Consumes one non-status wire line as this request's reply. Returns
    /// true when the line belonged to the pending ACK (the caller must not
    /// treat it as unsolicited chatter). Already-answered requests swallow
    /// further `ok` lines so a stale reply cannot clobber the recorded one.
    pub(super) fn route(&mut self, line: &str) -> bool {
        if self.reply.is_some() {
            // Keep the first reply; an extra `ok` would desynchronize anyway.
            return line == "ok";
        }
        if line == "ok" {
            self.reply = Some(StreamReply::Ok);
            true
        } else if let Some(detail) = line.strip_prefix("error:") {
            self.reply = Some(StreamReply::Error(detail.to_owned()));
            true
        } else if let Some(code) = line.strip_prefix("ALARM:") {
            self.reply = Some(StreamReply::Alarm(code.to_owned()));
            true
        } else {
            false
        }
    }
}

/// The three kinds of replies GRBL can answer a program line with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum StreamReply {
    /// Planner accepted the line.
    Ok,
    /// `error:<code>` rejection; the detail code follows the colon.
    Error(String),
    /// `ALARM:<n>` raised while the line was processed.
    Alarm(String),
}

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
    /// Firmware family the port was opened for; drives the command-length
    /// limit of [`Session`]-aware validation (task: FluidNC over COM port).
    protocol: SerialProtocol,
    rx_buffer: Vec<u8>,
    /// True until the startup handshake (soft reset + banner/status sync) has
    /// completed once. Opening the port toggles DTR/RTS, which resets most
    /// Arduino-based GRBL boards and makes them emit a boot banner; deferring
    /// the handshake to the first status query keeps `connect()` synchronous
    /// and non-blocking for the runtime that hosts the adapter.
    handshake_pending: bool,
    /// Number of position components the last `?` report carried (3 for GRBL,
    /// up to 6 for serial FluidNC — task: FluidNC over a COM port); seeds the
    /// axis list of `machine_configuration`. Mirrors the WebSocket adapter's
    /// `Session::reported_axis_count`.
    reported_axis_count: usize,
    /// Cached work offsets per coordinate system as six-slot arrays: GRBL
    /// fills X/Y/Z, serial FluidNC also the rotary slots, mirroring the
    /// status parser's internal position layout.
    work_offsets: HashMap<CoordinateSystem, [f64; 6]>,
    protocol_poisoned: bool,
    program_streaming: bool,
    /// One program line written to GRBL whose `ok`/`error` reply has not been
    /// observed yet; `None` while no line is in flight. See
    /// [`PendingProgramAck`] for the non-blocking streaming design.
    pending_ack: Option<PendingProgramAck>,
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
    /// Output interpretation inferred from `$32` for stock GRBL laser mode.
    spindle_mode: crate::SpindleMode,
    min_spindle_rpm: Option<f64>,
    max_spindle_rpm: Option<f64>,
    continuous_jog: Option<ContinuousJog>,
}

/// Serial-line firmware family spoken over the GRBL-style UART protocol.
///
/// Task (FluidNC over COM port): FluidNC builds on ESP32/S2/S3B expose a
/// UART that speaks the very same line protocol GRBL 1.x does — `ok`/`error:`
/// acknowledgements, realtime `?`/`0x18`/`0x85` bytes and `<State|MPos:…>`
/// status reports — so full control is implemented by ONE adapter with a
/// per-family behavior switch instead of a duplicated driver. The family is
/// decided at discovery time (the `[VER:FluidNC…]`/`[Boot:FluidNC…]` banner)
/// and propagated to `connect_fluidnc`; it then drives:
/// * the discovery id prefix (`grbl:serial:…` vs `fluidnc:serial:…`) so the
///   machines dialog highlights the correct row and Rosetta resolves the
///   correct backend dialect,
/// * the reported `protocol` label (`GRBL` / `FluidNC`),
/// * the accepted command line length (GRBL 72/80 bytes vs FluidNC 250) and
///   6-axis (X…C) status triplets,
/// * which axes the guards accept for jog/zero operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerialProtocol {
    /// Stock GRBL 1.1 (AVR boards: Uno/Nano/Mega).
    Grbl,
    /// FluidNC firmware reached over its UART0 console.
    FluidNc,
}

impl SerialProtocol {
    /// Discovery/device-id prefix for this family: the first segment of the
    /// `{prefix}:serial:{basename}` id consumed by Rosetta and the machines
    /// dialog. Must stay byte-identical to `cnc_core::discovery` conventions.
    pub fn id_prefix(self) -> &'static str {
        match self {
            Self::Grbl => "grbl",
            Self::FluidNc => "fluidnc",
        }
    }

    /// `DiscoveredDevice.protocol` label reported for this family.
    pub fn label(self) -> &'static str {
        match self {
            Self::Grbl => "GRBL",
            Self::FluidNc => "FluidNC",
        }
    }

    /// Maximum accepted length of one command line for this family. GRBL's
    /// UART RX buffer and line parser only fit [`super::MAX_COMMAND_LENGTH`]
    /// bytes; FluidNC accepts up to 250 bytes (configurable via `$Line`
    /// control settings), which matters for long jog/MDI lines.
    pub fn max_command_length(self) -> usize {
        match self {
            Self::Grbl => MAX_COMMAND_LENGTH,
            Self::FluidNc => 250,
        }
    }
}

/// GRBL-family controller reached through an async serial port. Also serves
/// FluidNC controllers plugged in over USB (`SerialProtocol::FluidNc`).
pub struct GrblAdapter {
    // The Tokio mutex serializes complete GRBL transactions, including I/O waits,
    // without blocking the runtime thread during await.
    session: Mutex<Session>,
    serial_endpoint: String,
    /// Firmware family this port was opened for (task: FluidNC over COM).
    protocol: SerialProtocol,
    /// Machine name reported by the discovery banner (`[MSG:Machine:…]`), kept
    /// so the adapter's own discovery rows match the machines-dialog row the
    /// operator clicked. GRBL boards report nothing and carry `None`.
    machine_name: Option<String>,
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
