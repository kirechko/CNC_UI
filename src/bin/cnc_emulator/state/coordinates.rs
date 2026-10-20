//! Manages active work-coordinate systems and persists their offsets.
use super::super::protocol::{api_error, WireError};
use super::Emulator;
use crate::cnc_api::*;
use std::fs;

impl Emulator {
    pub(super) fn select_coordinate_system(&mut self, system: CoordinateSystem) {
        let offset = self
            .status
            .coordinate_systems
            .iter()
            .find(|entry| entry.system == system)
            .map(|entry| entry.offset)
            .unwrap_or(Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                a: 0.0,
                b: 0.0,
            });

        self.status.active_coordinate_system = system;
        self.status.work_offset = offset;
        self.status.message = Some(format!("Active coordinate system: {}", system.label()));
    }

    pub(super) fn set_selected_work_zero(
        &mut self,
        system: CoordinateSystem,
    ) -> Result<(), WireError> {
        if self.active_move.is_some()
            || self.continuous_jog.is_some()
            || self.continuous_jog_xy.is_some()
            || self.status.homing
            || self.status.interpreter_state == InterpreterState::Reading
        {
            return Err(api_error(
                "motion_busy",
                "cannot set work zero while motion or program execution is active",
            ));
        }

        let offset = self.status.position;
        if let Some(entry) = self
            .status
            .coordinate_systems
            .iter_mut()
            .find(|entry| entry.system == system)
        {
            entry.offset = offset;
        } else {
            self.status
                .coordinate_systems
                .push(CoordinateSystemOffset { system, offset });
        }

        self.select_coordinate_system(system);
        self.persist_coordinate_offsets()?;
        self.status.message = Some(format!(
            "{} zero set at machine X{:.3} Y{:.3} Z{:.3}",
            system.label(),
            offset.x,
            offset.y,
            offset.z
        ));
        Ok(())
    }

    pub(super) fn set_axis_work_zero(&mut self, axis: Axis) -> Result<(), WireError> {
        if self.active_move.is_some()
            || self.continuous_jog.is_some()
            || self.continuous_jog_xy.is_some()
            || self.status.homing
            || self.status.interpreter_state == InterpreterState::Reading
        {
            return Err(api_error(
                "motion_busy",
                "cannot set work zero while motion or program execution is active",
            ));
        }

        let system = self.status.active_coordinate_system;
        let machine_position = self.status.position;
        let mut offset = self
            .status
            .coordinate_systems
            .iter()
            .find(|entry| entry.system == system)
            .map(|entry| entry.offset)
            .unwrap_or(Position {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                a: 0.0,
                b: 0.0,
            });

        match axis {
            Axis::X => offset.x = machine_position.x,
            Axis::Y => offset.y = machine_position.y,
            Axis::Z => offset.z = machine_position.z,
            Axis::A => offset.a = machine_position.a,
            Axis::B => offset.b = machine_position.b,
        }

        if let Some(entry) = self
            .status
            .coordinate_systems
            .iter_mut()
            .find(|entry| entry.system == system)
        {
            entry.offset = offset;
        } else {
            self.status
                .coordinate_systems
                .push(CoordinateSystemOffset { system, offset });
        }

        self.status.work_offset = offset;
        self.persist_coordinate_offsets()?;
        self.status.message = Some(format!(
            "{} {} zero set at machine position",
            system.label(),
            match axis {
                Axis::X => "X",
                Axis::Y => "Y",
                Axis::Z => "Z",
                Axis::A => "A",
                Axis::B => "B",
            }
        ));
        Ok(())
    }

    pub(super) fn persist_coordinate_offsets(&self) -> Result<(), WireError> {
        let yaml = serde_yaml_ng::to_string(&self.status.coordinate_systems)
            .map_err(|e| api_error("coordinates_save", e.to_string()))?;
        if let Some(parent) = self.coordinate_offsets_path.parent() {
            fs::create_dir_all(parent).map_err(|e| api_error("coordinates_save", e.to_string()))?;
        }
        fs::write(&self.coordinate_offsets_path, yaml)
            .map_err(|e| api_error("coordinates_save", e.to_string()))
    }
}
