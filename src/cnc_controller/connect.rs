//! Controller constructors: both spawn the `cnc-api-worker` thread that owns
//! the adapter and runs the tokio current-thread runtime.

use super::snapshot::{initial_disconnected_status, set_ui_alert};
use super::worker::api_worker;
// `Command` is not named here: the channel types are inferred at `mpsc::channel()`.
use super::CNCController;
use crate::cnc_api::{CncApi, CncResult};
use cnc_core::adapters::emulator::EmAdapter;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::Duration;

#[allow(dead_code)]
impl CNCController {
    /// Legacy constructor: keeps only the socket path and lets the worker
    /// retry `EmAdapter::connect` every second until the emulator appears.
    ///
    /// The UI window opens immediately because no network call happens here.
    pub fn connect(socket_path: impl AsRef<Path>) -> CncResult<Self> {
        let socket_path = socket_path.as_ref().to_path_buf();
        let status = Arc::new(Mutex::new(initial_disconnected_status()));
        let worker_status = status.clone();
        let (commands, receiver) = mpsc::channel();
        let (emergency_stop, emergency_receiver) = mpsc::channel();
        let command_cancel_requested = Arc::new(AtomicBool::new(false));
        let worker_command_cancel_requested = command_cancel_requested.clone();
        let motion_stop_requested = Arc::new(AtomicBool::new(false));
        let worker_motion_stop_requested = motion_stop_requested.clone();

        thread::Builder::new()
            .name("cnc-api-worker".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to create CNC API worker runtime");

                loop {
                    match EmAdapter::connect(&socket_path) {
                        Ok(api) => match runtime.block_on(api.status()) {
                            Ok(initial_status) => {
                                worker_status.lock().unwrap().machine = initial_status;
                                api_worker(
                                    Arc::new(api),
                                    receiver,
                                    emergency_receiver,
                                    worker_status.clone(),
                                    worker_command_cancel_requested,
                                    worker_motion_stop_requested,
                                    &runtime,
                                );
                                return;
                            }
                            Err(error) => {
                                set_ui_alert(&worker_status, &error.code, &error.message);
                                thread::sleep(Duration::from_secs(1));
                            }
                        },
                        Err(error) => {
                            set_ui_alert(&worker_status, &error.code, &error.message);
                            thread::sleep(Duration::from_secs(1));
                        }
                    }
                }
            })
            .map_err(|error| crate::cnc_api::ControllerError::new("worker", error.to_string()))?;

        Ok(Self {
            status,
            api: None,
            commands,
            emergency_stop,
            xy_jog_speed: Mutex::new(1000.0),
            z_jog_speed: Mutex::new(300.0),
            rotary_jog_speed: Mutex::new(300.0),
            spindle_speed_setpoint: Mutex::new(0.0),
            command_cancel_requested,
            motion_stop_requested,
        })
    }

    /// Preferred constructor used by the web backend and machine switching:
    /// wraps an already-built adapter (FluidNC WebSocket, emulator, …).
    pub fn connect_with_api(api: Arc<dyn CncApi>) -> CncResult<Self> {
        let status = Arc::new(Mutex::new(initial_disconnected_status()));
        let worker_status = status.clone();
        let (commands, receiver) = mpsc::channel();
        let (emergency_stop, emergency_receiver) = mpsc::channel();
        let command_cancel_requested = Arc::new(AtomicBool::new(false));
        let worker_command_cancel_requested = command_cancel_requested.clone();
        let motion_stop_requested = Arc::new(AtomicBool::new(false));
        let worker_motion_stop_requested = motion_stop_requested.clone();

        let worker_api = api.clone();
        thread::Builder::new()
            .name("cnc-api-worker".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to create CNC API worker runtime");

                match runtime.block_on(worker_api.status()) {
                    Ok(initial_status) => {
                        worker_status.lock().unwrap().machine = initial_status;
                    }
                    Err(error) => {
                        set_ui_alert(&worker_status, &error.code, &error.message);
                    }
                }

                api_worker(
                    worker_api,
                    receiver,
                    emergency_receiver,
                    worker_status,
                    worker_command_cancel_requested,
                    worker_motion_stop_requested,
                    &runtime,
                );
            })
            .map_err(|error| crate::cnc_api::ControllerError::new("worker", error.to_string()))?;

        Ok(Self {
            status,
            api: Some(api),
            commands,
            emergency_stop,
            xy_jog_speed: Mutex::new(1000.0),
            z_jog_speed: Mutex::new(300.0),
            rotary_jog_speed: Mutex::new(300.0),
            spindle_speed_setpoint: Mutex::new(0.0),
            command_cancel_requested,
            motion_stop_requested,
        })
    }
}
