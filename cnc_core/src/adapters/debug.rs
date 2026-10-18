use crate::{
    Axis, CncResult, CoordinateSystem, InterpreterState, JogRequest, LinuxCncApi, MachineStatus,
    Position, SpindleCommand, SpindleStatus, SpindleDirection, TaskMode, TaskState,
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
            direction: SpindleDirection::Stopped,
            enabled: false,
        },
        mist_coolant_on: false,
        flood_coolant_on: false,
        jog_rate: 0.0,
        motion_mode: None,
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

        #[async_trait::async_trait]
        impl LinuxCncApi for $adapter {
            async fn status(&self) -> CncResult<MachineStatus> {
                Ok(debug_status())
            }

            async fn power_on(&self) -> CncResult<()> {
                eprintln!(concat!("[", $label, "] power_on"));
                Ok(())
            }

            async fn power_off(&self) -> CncResult<()> {
                eprintln!(concat!("[", $label, "] power_off"));
                Ok(())
            }

            async fn estop(&self) -> CncResult<()> {
                eprintln!(concat!("[", $label, "] estop"));
                Ok(())
            }

            async fn home(&self, axis: Option<Axis>) -> CncResult<()> {
                eprintln!("[{}] home axis={axis:?}", $label);
                Ok(())
            }

            async fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()> {
                eprintln!("[{}] select_coordinate_system system={system:?}", $label);
                Ok(())
            }

            async fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()> {
                eprintln!("[{}] set_work_zero system={system:?}", $label);
                Ok(())
            }

            async fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()> {
                eprintln!("[{}] set_axis_work_zero axis={axis:?}", $label);
                Ok(())
            }

            async fn jog_increment(&self, request: JogRequest) -> CncResult<()> {
                eprintln!("[{}] jog_increment request={request:?}", $label);
                Ok(())
            }

            async fn jog_continuous(
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

            async fn jog_continuous_xy(
                &self,
                x_direction: i8,
                y_direction: i8,
                feed_rate: f64,
            ) -> CncResult<()> {
                eprintln!(
                    "[{}] jog_continuous_xy x_direction={x_direction} y_direction={y_direction} feed_rate={feed_rate}",
                    $label
                );
                Ok(())
            }

            async fn cancel_jog(&self) -> CncResult<()> {
                eprintln!(concat!("[", $label, "] cancel_jog"));
                Ok(())
            }

            async fn mdi(&self, command: &str) -> CncResult<()> {
                eprintln!("[{}] mdi command={command:?}", $label);
                Ok(())
            }

            async fn load_program(&self, path: &Path) -> CncResult<()> {
                eprintln!("[{}] load_program path={}", $label, path.display());
                Ok(())
            }

            async fn program_run(&self, start_line: u32) -> CncResult<()> {
                eprintln!("[{}] program_run start_line={start_line}", $label);
                Ok(())
            }

            async fn program_pause(&self) -> CncResult<()> {
                eprintln!(concat!("[", $label, "] program_pause"));
                Ok(())
            }

            async fn program_resume(&self) -> CncResult<()> {
                eprintln!(concat!("[", $label, "] program_resume"));
                Ok(())
            }

            async fn program_abort(&self) -> CncResult<()> {
                eprintln!(concat!("[", $label, "] program_abort"));
                Ok(())
            }

            async fn set_spindle(&self, command: SpindleCommand) -> CncResult<()> {
                eprintln!("[{}] set_spindle command={command:?}", $label);
                Ok(())
            }

            async fn clear_error(&self) -> CncResult<()> {
                eprintln!(concat!("[", $label, "] clear_error"));
                Ok(())
            }
        }
    };
}

debug_adapter!(LinuxCncAdapter, "LinuxCncAdapter");
debug_adapter!(FluidNcAdapter, "FluidNcAdapter");
