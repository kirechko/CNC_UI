//! GRBL 1.1 serial adapter: a blocking `serialport`-backed implementation of the
//! legacy `LinuxCncApi` trait. E-Stop must be provided by the machine's hardware;
//! GRBL's real-time feed hold is deliberately not presented as an E-Stop.
//!
//! NOTE: this module tree is legacy pre-`cnc_core` code. It is not declared by any
//! binary in the crate (`main.rs`, `cnc-web.rs`, `cnc-emulator.rs`), so it does not
//! participate in the current build graph. It is kept, split into children purely
//! to satisfy the file-size policy, for archival reference.
//!
//! Children:
//! - [`transport`] - byte-level serial line I/O, stop-and-wait command exchange,
//!   realtime character injection, and the status/settings queries.
//! - [`status`] - GRBL status-report parsing plus coordinate/axis formatting helpers
//!   used when building G-code commands.
//! - [`api`] - the `impl LinuxCncApi for GrblAdapter` block that ties everything
//!   together into the machine-control surface.

use crate::{ControllerError, CncResult, CoordinateSystem, SpindleCommand, SpindleDirection};
use serialport::SerialPort;
use std::sync::Mutex;
use std::time::Duration;

mod api;
mod status;
mod transport;

/// How long a regular (line-based) command may wait for its `ok`/`error:` acknowledgement.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(3);
/// How long the realtime `?` status query may wait for a `<...>` report line.
const STATUS_TIMEOUT: Duration = Duration::from_secs(1);
/// Hard cap for a single response line read from the serial port.
const MAX_RESPONSE_LINE: usize = 4096;

/// A pending continuous XY jog: direction signs and the feed rate the chunks use.
#[derive(Clone, Copy)]
struct ContinuousXyJog {
    x_direction: i8,
    y_direction: i8,
    feed_rate: f64,
}

/// Everything the adapter owns behind the mutex: the serial port itself plus the
/// host-side bookkeeping (loaded program, spindle setpoint, active coordinate
/// system, and the continuous-jog chunk pump state).
struct Session {
    port: Box<dyn SerialPort>,
    program: Vec<String>,
    program_name: Option<String>,
    current_line: u32,
    active_coordinate_system: CoordinateSystem,
    spindle_command: SpindleCommand,
    continuous_xy_jog: Option<ContinuousXyJog>,
}

/// GRBL 1.1 Serial adapter. E-Stop must be provided by the machine's hardware;
/// GRBL's real-time feed hold is deliberately not presented as an E-Stop.
pub struct GrblAdapter {
    session: Mutex<Session>,
}

impl GrblAdapter {
    /// Opens the serial port and initializes the session with GRBL power-on defaults
    /// (G54 coordinate system, stopped spindle, no loaded program).
    pub fn connect(port_name: &str, baud_rate: u32) -> CncResult<Self> {
        let port = serialport::new(port_name, baud_rate)
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|error| ControllerError::new("connect", error.to_string()))?;

        Ok(Self {
            session: Mutex::new(Session {
                port,
                program: Vec::new(),
                program_name: None,
                current_line: 0,
                active_coordinate_system: CoordinateSystem::G54,
                spindle_command: SpindleCommand {
                    direction: SpindleDirection::Stopped,
                    rpm: 0.0,
                },
                continuous_xy_jog: None,
            }),
        })
    }

    /// Acquires the session lock, mapping a poisoned mutex to a stable error code.
    fn lock_session(&self) -> CncResult<std::sync::MutexGuard<'_, Session>> {
        self.session
            .lock()
            .map_err(|_| ControllerError::new("connection", "GRBL session lock poisoned"))
    }
}
