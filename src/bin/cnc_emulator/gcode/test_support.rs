//! Provides a consistent emulator configuration for G-code unit tests.
use super::super::config::{AxesConfig, AxisLimits, EmulatorConfig, SpindleConfig};
use super::super::state::Emulator;
use crate::cnc_api::{CoordinateSystem, CoordinateSystemOffset, Position, SpindleMode};
use std::path::PathBuf;

/// Builds a deterministic default-spindle emulator fixture for parser and G-code tests.
pub fn test_emulator() -> Emulator {
    // This fixture deliberately starts with a conventional spindle; laser-specific tests
    // opt into an explicit LaserConfiguration so setup defaults cannot mask safety paths.
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
            mode: SpindleMode::Standard,
            laser: None,
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
            c: 0.0,
        },
    }];

    // Tests share one known-zero G54 frame so assertions exercise parser behavior, not setup I/O.
    Emulator::new(config, PathBuf::from("unused-test-offsets.yaml"), offsets)
}
