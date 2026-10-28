//! `AppState`: the long-lived backend state shared by axum handlers and the
//! Tauri commands, plus the high-level operations they invoke.
//!
//! The struct definition and constructor live here; the SQLite-backed
//! operations are in `log_ops.rs` and the coordinate/zero/home/command
//! operations in `position.rs`.

use rusqlite::Connection;
use std::io;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use super::machine::{default_controller, emulator_device};
use super::models::LoadedProgram;
use crate::cnc_controller::CNCController;

/// Shared backend state: controller handle, loaded program, message-log
/// database, discovery registry and the cancellation flags of the *current*
/// controller worker pair.
pub(crate) struct AppState {
    pub(crate) controller: Mutex<CNCController>,
    pub(crate) program: Mutex<Option<LoadedProgram>>,
    pub(crate) log: Mutex<Connection>,
    pub(crate) last_logged_status: Mutex<Option<String>>,
    /// Latest FluidNC/network discovery registry. The local emulator entry is
    /// never stored here: it is probed live whenever a machine snapshot is
    /// built, so a crashed or stopped emulator disappears from the list
    /// automatically. Future scanners (serial COM ports, LinuxCNC) contribute
    /// their entries to this same registry.
    pub(crate) machines: Mutex<Vec<cnc_core::discovery::DiscoveredDevice>>,
    /// One-at-a-time guard for background scans triggered by GET requests, so
    /// a polling browser cannot stack parallel network sweeps.
    pub(crate) discovery_in_flight: Arc<AtomicBool>,
    /// Cancellation handles of the *current* controller. They are swapped in
    /// `Mutex` wrappers whenever the user switches machines, while any older
    /// worker threads keep observing the handles they were started with.
    pub(crate) command_cancel_requested: Mutex<Arc<AtomicBool>>,
    pub(crate) motion_stop_requested: Mutex<Arc<AtomicBool>>,
}

impl AppState {
    pub(crate) fn new() -> io::Result<Self> {
        // No adapter configuration file anymore: the backend always boots with
        // the lazily connecting emulator adapter, and every other machine
        // (FluidNC on the network, later COM ports and LinuxCNC) becomes
        // reachable through the discovery list in the machines dialog.
        let controller = default_controller()?;
        let command_cancel_requested = controller.command_cancellation_handle();
        let motion_stop_requested = controller.motion_stop_handle();
        let state = Self {
            controller: Mutex::new(controller),
            program: Mutex::new(None),
            log: Mutex::new(super::log_store::open_log()?),
            last_logged_status: Mutex::new(None),
            machines: Mutex::new(Vec::new()),
            discovery_in_flight: Arc::new(AtomicBool::new(false)),
            command_cancel_requested: Mutex::new(command_cancel_requested),
            motion_stop_requested: Mutex::new(motion_stop_requested),
        };
        state.load_default_program();
        Ok(state)
    }

    /// Locks the controller and reports the connected device id only while the
    /// status cache actually says the machine link is up.
    pub(crate) fn connected_machine_id(&self) -> Option<String> {
        let controller = self.controller.lock().ok()?;
        controller
            .get_status_snapshot()
            .machine
            .connected
            .then(|| controller.connected_device_id())
            .flatten()
    }

    /// Full machine list served to the UI: the live emulator entry first,
    /// followed by the FluidNC devices cached in the discovery registry.
    /// Future scanners (COM ports, LinuxCNC) only need to contribute entries
    /// to the registry or the probe used here.
    pub(crate) fn machine_snapshot(&self) -> Vec<cnc_core::discovery::DiscoveredDevice> {
        let mut devices = emulator_device().into_iter().collect::<Vec<_>>();
        if let Ok(registry) = self.machines.lock() {
            for device in &*registry {
                if !devices.iter().any(|listed| listed.id == device.id) {
                    devices.push(device.clone());
                }
            }
        }
        devices
    }

    /// Replaces the running controller with a new one and publishes the
    /// cancellation handles of the new worker pair. Dropping the old
    /// `CNCController` closes its command channels, so its worker thread
    /// exits through `RecvTimeoutError::Disconnected`.
    pub(crate) fn swap_controller(&self, controller: CNCController) -> Result<(), String> {
        let command_cancel_handle = controller.command_cancellation_handle();
        let motion_stop_handle = controller.motion_stop_handle();
        {
            let mut slot = self
                .controller
                .lock()
                .map_err(|_| "controller lock poisoned".to_string())?;
            *slot = controller;
        }
        *self
            .command_cancel_requested
            .lock()
            .map_err(|_| "cancel handle lock poisoned".to_string())? = command_cancel_handle;
        *self
            .motion_stop_requested
            .lock()
            .map_err(|_| "stop handle lock poisoned".to_string())? = motion_stop_handle;
        Ok(())
    }
}

// Sub-implementation files of AppState; `position::temp_program_path` is also
// used by the program-upload module.
pub(crate) mod log_ops;
pub(crate) mod position;
