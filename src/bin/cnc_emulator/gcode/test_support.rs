//! Provides a consistent emulator configuration for G-code unit tests.
use super::super::config::{AxesConfig, AxisLimits, EmulatorConfig, SpindleConfig};
use super::super::state::Emulator;
use crate::cnc_api::{CoordinateSystem, CoordinateSystemOffset, Position};
use std::path::PathBuf;

pub fn test_emulator() -> Emulator {
    let config = EmulatorConfig {
        axes: AxesConfig {
            x: AxisLimits {
                min_mm: 0.0,
                max_mm: 300.0,
                max_velocity_mm_min: 6000.0,
                acceleration_mm_s2: 100.0,
            },
            y: AxisLimits {
                min_mm: 0.0,
                max_mm: 400.0,
                max_velocity_mm_min: 6000.0,
                acceleration_mm_s2: 100.0,
            },
            z: Some(AxisLimits {
                min_mm: 0.0,
                max_mm: 100.0,
                max_velocity_mm_min: 3000.0,
                acceleration_mm_s2: 50.0,
            }),
            a: None,
            b: None,
        },
        spindle: SpindleConfig {
            min_rpm: 500.0,
            max_rpm: 24_000.0,
            acceleration_rpm_per_sec: 1000.0,
        },
    };
    let offsets = vec![CoordinateSystemOffset {
        system: CoordinateSystem::G54,
        offset: Position {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            a: 0.0,
            b: 0.0,
        },
    }];

    Emulator::new(config, PathBuf::from("unused-test-offsets.yaml"), offsets)
}
