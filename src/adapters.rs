use crate::cnc_api::{
    Axis, CncResult, CoordinateSystem, InterpreterState, JogRequest, LinuxCncApi, MachineStatus,
    Position, SpindleCommand, SpindleStatus, TaskMode, TaskState,
};
use std::path::Path;

fn debug_status() -> MachineStatus {
    MachineStatus {
        connected: true,
        task_mode: TaskMode::Manual,
        task_state: TaskState::On,
        interpreter_state: InterpreterState::Idle,
        position: Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            a: 0.0,
            b: 0.0,
        },
        work_offset: Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            a: 0.0,
            b: 0.0,
        },
        active_coordinate_system: CoordinateSystem::G54,
        coordinate_systems: Vec::new(),
        feed_rate: 0.0,
        spindle: SpindleStatus {
            commanded_rpm: 0.0,
            actual_rpm: 0.0,
            direction: crate::cnc_api::SpindleDirection::Stopped,
            enabled: false,
        },
        mist_coolant_on: false,
        flood_coolant_on: false,
        max_spindle_rpm: 0.0,
        loaded_program: None,
        current_line: 0,
        total_lines: 0,
        message: Some("Debug adapter: no backend logic".into()),
        homed: [false; 5],
        homing: false,
    }
}

macro_rules! debug_adapter {
    ($adapter:ident, $label:literal) => {
        pub struct $adapter;

        impl $adapter {
            pub fn new() -> Self {
                Self
            }
        }

        impl LinuxCncApi for $adapter {
            fn status(&self) -> CncResult<MachineStatus> {
                Ok(debug_status())
            }

            fn power_on(&self) -> CncResult<()> {
                eprintln!(concat!("[", $label, "] power_on"));
                Ok(())
            }

            fn power_off(&self) -> CncResult<()> {
                eprintln!(concat!("[", $label, "] power_off"));
                Ok(())
            }

            fn estop(&self) -> CncResult<()> {
                eprintln!(concat!("[", $label, "] estop"));
                Ok(())
            }

            fn home(&self, axis: Option<Axis>) -> CncResult<()> {
                eprintln!("[{}] home axis={axis:?}", $label);
                Ok(())
            }

            fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()> {
                eprintln!("[{}] select_coordinate_system system={system:?}", $label);
                Ok(())
            }

            fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()> {
                eprintln!("[{}] set_work_zero system={system:?}", $label);
                Ok(())
            }

            fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()> {
                eprintln!("[{}] set_axis_work_zero axis={axis:?}", $label);
                Ok(())
            }

            fn jog_increment(&self, request: JogRequest) -> CncResult<()> {
                eprintln!("[{}] jog_increment request={request:?}", $label);
                Ok(())
            }

            fn jog_continuous(
                &self,
                axis: Axis,
                direction: i8,
                feed_rate: f64,
            ) -> CncResult<()> {
                eprintln!(
                    "[{}] jog_continuous axis={axis:?} direction={direction} feed_rate={feed_rate}",
                    $label
                );
                Ok(())
            }

            fn mdi(&self, command: &str) -> CncResult<()> {
                eprintln!("[{}] mdi command={command:?}", $label);
                Ok(())
            }

            fn load_program(&self, path: &Path) -> CncResult<()> {
                eprintln!("[{}] load_program path={}", $label, path.display());
                Ok(())
            }

            fn program_run(&self, start_line: u32) -> CncResult<()> {
                eprintln!("[{}] program_run start_line={start_line}", $label);
                Ok(())
            }

            fn program_pause(&self) -> CncResult<()> {
                eprintln!(concat!("[", $label, "] program_pause"));
                Ok(())
            }

            fn program_resume(&self) -> CncResult<()> {
                eprintln!(concat!("[", $label, "] program_resume"));
                Ok(())
            }

            fn program_abort(&self) -> CncResult<()> {
                eprintln!(concat!("[", $label, "] program_abort"));
                Ok(())
            }

            fn set_spindle(&self, command: SpindleCommand) -> CncResult<()> {
                eprintln!("[{}] set_spindle command={command:?}", $label);
                Ok(())
            }

            fn clear_error(&self) -> CncResult<()> {
                eprintln!(concat!("[", $label, "] clear_error"));
                Ok(())
            }
        }
    };
}

debug_adapter!(LinuxCncAdapter, "LinuxCncAdapter");
debug_adapter!(FluidNcAdapter, "FluidNcAdapter");
