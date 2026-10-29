//! Axum HTTP backend serving the browser UI and the JSON control API.
//!
//! The implementation used to live in a single 2300-line file; it is now a
//! directory module so every layer of the HTTP contract stays reviewable:
//!
//! - `models`    — JSON request/response types shared with the browser and the
//!                 Tauri commands (field names are a compatibility contract,
//!                 see the `contract_tests` module).
//! - `state`     — `AppState`: controller handle, loaded program, SQLite log
//!                 database, discovery registry and the high-level operations
//!                 the handlers run (split into `state/log_ops.rs` and
//!                 `state/position.rs`).
//! - `log_store` — raw rusqlite statements for the message log, command
//!                 history and the discovered-endpoint cache.
//! - `machine`   — local emulator probing and adapter-construction helpers.
//! - `discovery` — machine-list endpoints: instant snapshot, SSE rescan and
//!                 runtime machine switching.
//! - `program`   — G-code upload validation, temp saving and loading.
//! - `handlers`  — thin axum handler wrappers (`control` for jog/spindle/
//!                 position, `console` for commands/log/program endpoints).
//! - `server`    — router assembly, address selection and the static-asset
//!                 cache-control middleware.
//!
//! `src/tauri_app.rs` keeps importing through `crate::web_server::...`; the
//! re-exports below preserve exactly the names of the old flat module.

mod discovery;
mod files;
mod handlers;
mod log_store;
mod machine;
mod models;
mod program;
mod server;
mod state;

pub use server::run;

// Re-exports for `src/tauri_app.rs`: the Tauri desktop commands share the same
// models and state operations as the axum handlers, so the names exported by
// the original flat `web_server.rs` remain reachable through this module.
// The axum binary reaches these items through the submodule paths directly,
// so `unused_imports` is allowed for builds that exclude the Tauri binary.
#[allow(unused_imports)]
pub(crate) use models::{
    validate_macros, LatestLogMessages, LoadedProgram, LogEntry, LogPage, LogQuery,
    MacroDefinition, MacroInvocation, StatusResponse,
};
#[allow(unused_imports)]
pub(crate) use program::{load_program, ProgramError};
pub(crate) use state::AppState;

use std::sync::Arc;

/// Shared handle passed to every axum handler (`Router::with_state`).
pub(crate) type SharedController = Arc<AppState>;

#[cfg(test)]
mod contract_tests {
    // `control` itself is private to `handlers`; the two request types are
    // reachable through the `pub(crate) use control::*` re-export instead.
    use super::handlers::{ContinuousXyJogRequest, XyJogRequest};
    use super::models::*;
    use super::program::{ProgramError, ProgramRequest};
    use crate::cnc_api::{
        CoordinateSystem, InterpreterState, MachineStatus, Position, SpindleStatus, TaskMode,
        TaskState,
    };
    use crate::cnc_controller::ControllerSnapshot;

    fn sample_status() -> ControllerSnapshot {
        ControllerSnapshot {
            machine: MachineStatus {
                connected: true,
                task_mode: TaskMode::Manual,
                task_state: TaskState::On,
                interpreter_state: InterpreterState::Idle,
                position: Position {
                    x: 11.0,
                    y: 12.0,
                    z: 13.0,
                    a: 14.0,
                    b: 15.0,
                    c: 16.0,
                },
                work_offset: Position {
                    x: 10.0,
                    y: 10.0,
                    z: 10.0,
                    a: 10.0,
                    b: 10.0,
                    c: 10.0,
                },
                active_coordinate_system: CoordinateSystem::G59_1,
                coordinate_systems: Vec::new(),
                feed_rate: 100.0,
                spindle: SpindleStatus {
                    commanded_rpm: 0.0,
                    actual_rpm: 0.0,
                    direction: crate::cnc_api::SpindleDirection::Stopped,
                    enabled: false,
                },
                mist_coolant_on: false,
                flood_coolant_on: false,
                jog_rate: 0.0,
                motion_mode: None,
                max_spindle_rpm: 24_000.0,
                loaded_program: None,
                current_line: 0,
                total_lines: 0,
                message: Some("ready".into()),
                homed: [false; 6],
                homing: false,
                progress: cnc_core::ToolpathProgress::default(),
            },
            machine_configuration: None,
            jog_step: 1.0,
            ui_alert: None,
            toolpath: cnc_core::Toolpath::default(),
        }
    }

    #[test]
    fn macro_definitions_preserve_frontend_metadata_names_and_defaults() {
        let macro_definition: MacroDefinition = serde_json::from_str(
            r#"{"id":"touch-off","name":"Touch off","description":"Probe","body":"G38.2 Z-5 F50","invocations":[{"controller":"LinuxCNC","mechanism":"M-code","command":"M101"}]}"#,
        )
        .expect("deserialize macro definition");
        assert_eq!(macro_definition.id, "touch-off");
        assert_eq!(macro_definition.invocations[0].command, "M101");

        let encoded = serde_json::to_value(&macro_definition).expect("serialize macro definition");
        assert_eq!(encoded["invocations"][0]["controller"], "LinuxCNC");
        assert_eq!(encoded["invocations"][0]["mechanism"], "M-code");
    }

    #[test]
    fn macro_validation_rejects_duplicate_or_unsafe_invocations() {
        let valid = MacroDefinition {
            id: "touch-off".into(),
            name: "Touch off".into(),
            description: String::new(),
            body: "G38.2 Z-5 F50".into(),
            invocations: vec![
                MacroInvocation {
                    controller: "LinuxCNC".into(),
                    mechanism: "M-code".into(),
                    command: "M101".into(),
                },
                MacroInvocation {
                    controller: "linuxcnc".into(),
                    mechanism: "M-code".into(),
                    command: "M102".into(),
                },
            ],
        };
        assert!(validate_macros(&[valid], "en").is_err());
    }

    #[test]
    fn status_response_preserves_frontend_json_field_names() {
        let value = serde_json::to_value(StatusResponse::from(sample_status())).unwrap();

        for field in [
            "connected",
            "state",
            "x",
            "y",
            "z",
            "a",
            "b",
            "c",
            "machine_x",
            "machine_y",
            "machine_z",
            "machine_a",
            "machine_b",
            "machine_c",
            "coordinate_system",
            "configuration",
            "movement_speed",
            "movement_mode",
            "spindle_rpm",
            "spindle_commanded_rpm",
            "spindle_enabled",
            "spindle_direction",
            "spindle_min_rpm",
            "spindle_max_rpm",
            "current_line",
            "total_lines",
            "message",
            "error",
        ] {
            assert!(value.get(field).is_some(), "missing frontend field {field}");
        }

        assert_eq!(value["coordinate_system"], "g59_1");
        assert_eq!(value["x"], 1.0);
        assert_eq!(value["machine_x"], 11.0);
    }

    #[test]
    fn frontend_json_request_names_deserialize_unchanged() {
        let program: ProgramRequest = serde_json::from_value(serde_json::json!({
            "fileName": "part.ngc",
            "content": "G0 X1"
        }))
        .unwrap();
        assert_eq!(program.file_name, "part.ngc");

        let jog: XyJogRequest = serde_json::from_value(serde_json::json!({
            "xDirection": 1,
            "yDirection": -1,
            "distance": 0.1
        }))
        .unwrap();
        assert_eq!((jog.x_direction, jog.y_direction), (1, -1));
        let _ = (
            ProgramError::client("x".into()),
            ProgramError::internal("y".into()),
        );

        let continuous_jog: ContinuousXyJogRequest = serde_json::from_value(serde_json::json!({
            "xDirection": 1,
            "yDirection": 0
        }))
        .unwrap();
        assert_eq!(
            (continuous_jog.x_direction, continuous_jog.y_direction),
            (1, 0)
        );
    }

    #[test]
    fn loaded_program_keeps_frontend_json_shape() {
        let value = serde_json::to_value(LoadedProgram {
            name: "part.ngc".into(),
            content: "G0 X1".into(),
        })
        .unwrap();

        assert_eq!(
            value,
            serde_json::json!({
                "name": "part.ngc",
                "content": "G0 X1"
            })
        );
    }
}
