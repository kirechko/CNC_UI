//! GRBL-family machine configuration assembled from controller settings.

use super::super::{GrblAdapter, SerialProtocol};
use crate::{
    AxisConfiguration, AxisUnits, CncResult, MachineAxis, MachineConfiguration,
    SpindleConfiguration,
};

impl GrblAdapter {
    /// Reads `$$` settings and maps the GRBL $1xx/$3x/$13x numbering into
    /// per-axis travel/velocity/acceleration and spindle RPM limits.
    ///
    /// For serial FluidNC (task: FluidNC over a COM port) the configuration
    /// follows the WebSocket adapter's convention: $130-$135/$110-$115/
    /// $120-$125/$100-$105 are the X/Y/Z/A/B/C slots and the number of
    /// reported axes comes from the last `?` status poll, so a 4/5/6-axis
    /// machine exposes its rotary axes to the UI.
    pub(super) async fn api_machine_configuration(&self) -> CncResult<MachineConfiguration> {
        let mut session = self.session.lock().await;
        let settings = Self::query_settings(&mut session).await?;
        // GRBL $32 enables laser behavior; the adapter's runtime mode tracks whether
        // the operator selected M3 constant power or M4 dynamic power.
        let laser_enabled = self.protocol == SerialProtocol::Grbl
            && settings
                .get(&32)
                .is_some_and(|laser_mode| *laser_mode == 1.0);
        let spindle_mode = if laser_enabled {
            match session.spindle_mode {
                crate::SpindleMode::LaserDynamic => crate::SpindleMode::LaserDynamic,
                _ => crate::SpindleMode::LaserConstant,
            }
        } else {
            crate::SpindleMode::Standard
        };
        session.spindle_mode = spindle_mode;
        let travel = |number: u16| settings.get(&number).copied().filter(|value| *value > 0.0);
        let spindle_rpm =
            |number: u16| settings.get(&number).copied().filter(|value| *value >= 0.0);
        let min_rpm = spindle_rpm(31);
        let max_rpm =
            spindle_rpm(30).filter(|maximum| min_rpm.is_none_or(|minimum| minimum <= *maximum));
        session.min_spindle_rpm = min_rpm;
        session.max_spindle_rpm = max_rpm;

        let axis_definitions = [
            (MachineAxis::X, AxisUnits::Linear),
            (MachineAxis::Y, AxisUnits::Linear),
            (MachineAxis::Z, AxisUnits::Linear),
            (MachineAxis::A, AxisUnits::Angular),
            (MachineAxis::B, AxisUnits::Angular),
            (MachineAxis::C, AxisUnits::Angular),
        ];
        // GRBL boards are 3-axis by firmware design; serial FluidNC reports
        // its real axis count via the MPos/WPos width of the status line.
        let axis_count = match self.protocol {
            SerialProtocol::Grbl => 3,
            SerialProtocol::FluidNc => session.reported_axis_count.clamp(3, 6),
        };
        let axes = axis_definitions
            .into_iter()
            .take(axis_count)
            .enumerate()
            .map(|(index, (axis, units))| AxisConfiguration {
                axis,
                units,
                max_travel: travel(130 + index as u16),
                max_velocity: travel(110 + index as u16),
                acceleration: travel(120 + index as u16),
                steps_per_unit: travel(100 + index as u16),
            })
            .collect();

        let spindle = if spindle_mode == crate::SpindleMode::Standard {
            SpindleConfiguration {
                min_rpm,
                max_rpm,
                mode: spindle_mode,
                laser: None,
            }
        } else {
            let maximum = max_rpm.unwrap_or(1000.0);
            SpindleConfiguration {
                min_rpm: Some(0.0),
                max_rpm: Some(maximum),
                mode: spindle_mode,
                laser: Some(crate::LaserConfiguration {
                    min_power: 0.0,
                    max_power: maximum,
                    pwm_frequency_hz: None,
                    dynamic_power_supported: true,
                    auto_shutoff_on_pause: true,
                }),
            }
        };
        Ok(MachineConfiguration {
            axes,
            spindle,
            settings,
        })
    }
}
