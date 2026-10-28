//! Coordinate-system, zeroing/homing and program-start operations of
//! [`AppState`]. Errors returned here are translated client-facing strings
//! (the `language` argument comes from the request's `lang`/`Accept-Language`).

use std::fs;
use std::sync::atomic::Ordering;

use super::AppState;
use crate::cnc_api::Axis;
use crate::web_server::models::{
    axis_letter, coordinate_system_number, parse_coordinate_system, LoadedProgram,
};

/// Monotonic id appended to every temp program file so concurrent uploads get
/// distinct paths.
static UPLOADED_PROGRAM_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Allocates a unique temp path for an uploaded program and ensures the
/// directory exists.
pub(crate) fn temp_program_path() -> std::path::PathBuf {
    let directory = std::env::temp_dir().join("cnc-ui-programs");
    let _ = fs::create_dir_all(&directory);
    let id = UPLOADED_PROGRAM_ID.fetch_add(1, Ordering::Relaxed);
    directory.join(format!("program-{}-{id}.ngc", std::process::id()))
}

impl AppState {
    /// Requests cancellation of the running machine command and stops motion.
    /// Lock order is fixed (command handle first, then motion handle), and the
    /// guards are cloned flags: nothing here ever takes the controller lock,
    /// so machine switches running in parallel cannot deadlock.
    pub(crate) fn cancel_machine_command(&self) {
        let command_cancel = self
            .command_cancel_requested
            .lock()
            .map(|handle| handle.clone())
            .unwrap_or_default();
        let motion_stop = self
            .motion_stop_requested
            .lock()
            .map(|handle| handle.clone())
            .unwrap_or_default();
        command_cancel.store(true, Ordering::Release);
        motion_stop.store(true, Ordering::Release);
    }

    /// Selects the active coordinate system; `language` localizes any error.
    pub(crate) fn select_coordinate_system(&self, key: &str, language: &str) -> Result<(), String> {
        let system = parse_coordinate_system(key, language)?;
        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller.select_coordinate_system(system);
        Ok(())
    }

    /// Sets the work zero of all axes; `language` localizes any error.
    pub(crate) fn zero_all(&self, key: &str, language: &str) -> Result<(), String> {
        let system = parse_coordinate_system(key, language)?;
        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller.set_work_zero(system);
        Ok(())
    }

    pub(crate) fn zero_axis(&self, axis: Axis) -> Result<(), String> {
        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller.zero_axis(axis);
        Ok(())
    }

    pub(crate) fn home_axis(&self, axis: Axis) -> Result<(), String> {
        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller.home_axis(axis);
        Ok(())
    }

    pub(crate) fn home_all(&self) -> Result<(), String> {
        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller.home_all();
        Ok(())
    }

    /// Writes a work coordinate for one axis; `language` localizes any error.
    pub(crate) fn set_axis_work_value(
        &self,
        axis: Axis,
        key: &str,
        value: f64,
        language: &str,
    ) -> Result<(), String> {
        if !value.is_finite() || value.abs() > 100_000.0 {
            return Err(crate::i18n::translate(language, "api.error.coordinateRange", &[]));
        }

        let system = parse_coordinate_system(key, language)?;
        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        if !controller.get_status_snapshot().machine.connected {
            return Err(crate::i18n::translate(language, "api.error.noConnection", &[]));
        }

        let command = format!(
            "G10 L2 P{} {}{value:.6}",
            coordinate_system_number(system),
            axis_letter(axis)
        );
        controller.send_command(&command);
        Ok(())
    }

    /// Moves one axis to a work coordinate; `language` localizes any error.
    pub(crate) fn go_to_axis_work_value(
        &self,
        axis: Axis,
        key: &str,
        value: f64,
        language: &str,
    ) -> Result<(), String> {
        if !value.is_finite() || value.abs() > 100_000.0 {
            return Err(crate::i18n::translate(language, "api.error.coordinateRange", &[]));
        }

        let system = parse_coordinate_system(key, language)?;
        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        let status = controller.get_status_snapshot();
        if !status.machine.connected {
            return Err(crate::i18n::translate(language, "api.error.noConnection", &[]));
        }

        let feed_rate = if status.machine.feed_rate.is_finite() && status.machine.feed_rate > 0.0 {
            status.machine.feed_rate
        } else {
            100.0
        };
        let command = format!(
            "{} G90 G1 {}{value:.6} F{feed_rate:.0}",
            system.label(),
            axis_letter(axis)
        );
        controller.send_command(&command);
        Ok(())
    }

    /// Starts the currently loaded program; `language` localizes client-facing errors.
    pub(crate) fn start_loaded_program(&self, language: &str) -> Result<(), String> {
        let program = self
            .program
            .lock()
            .map_err(|_| "program lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| crate::i18n::translate(language, "api.error.noProgram", &[]))?;

        if !program
            .content
            .lines()
            .any(|line| !line.split(';').next().unwrap_or("").trim().is_empty())
        {
            return Err(crate::i18n::translate(language, "api.error.emptyProgram", &[]));
        }

        let path = temp_program_path();
        fs::write(&path, &program.content)
            .map_err(|error| format!("Failed to restore G-code: {error}"))?;

        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller
            .load_program(&path)
            .map_err(|error| format!("{}: {}", error.code, error.message))?;
        controller
            .start_program()
            .map_err(|error| format!("{}: {}", error.code, error.message))
    }

    /// Reads `test.gcode` from the working directory into the program state
    /// (best-effort: a missing file only logs a message) so a fresh start of
    /// the backend already shows a trajectory in the viewer.
    pub(crate) fn load_default_program(&self) {
        let default_path = std::path::PathBuf::from("test.gcode");

        match fs::read_to_string(&default_path) {
            Ok(content) => {
                if let Ok(controller) = self.controller.lock() {
                    if let Err(error) = controller.load_program(&default_path) {
                        eprintln!(
                            "Could not load default program {}: {}: {}",
                            default_path.display(),
                            error.code,
                            error.message
                        );
                    } else if let Ok(mut program) = self.program.lock() {
                        *program = Some(LoadedProgram {
                            name: default_path
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("test.gcode")
                                .to_string(),
                            content,
                        });
                        eprintln!(
                            "Loaded default G-code trajectory from {}",
                            default_path.display()
                        );
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                eprintln!(
                    "Default G-code file {} not found; starting without a trajectory",
                    default_path.display()
                );
            }
            Err(error) => {
                eprintln!(
                    "Could not read default G-code file {}: {error}",
                    default_path.display()
                );
            }
        }
    }
}
