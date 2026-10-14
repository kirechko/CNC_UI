 //! Defines the toolpath, camera, and tool-telemetry data used by the viewer.
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum MoveType {
    Rapid,
    Linear,
    ArcClockwise,
    ArcCounterClockwise,
}

#[derive(Debug, Clone)]
pub(super) struct Segment {
    pub(super) x_start: f64,
    pub(super) y_start: f64,
    pub(super) z_start: f64,
    pub(super) x_end: f64,
    pub(super) y_end: f64,
    pub(super) z_end: f64,
    pub(super) move_type: MoveType,
}

#[derive(Clone)]
pub(super) struct CameraOrientation {
    pub(super) right: [f64; 3],
    pub(super) up: [f64; 3],
    pub(super) fwd: [f64; 3],
}

#[derive(Clone)]
pub(super) struct ToolVisual {
    pub(super) x: f64,
    pub(super) y: f64,
    pub(super) z: f64,
    pub(super) rpm: f64,
    pub(super) max_rpm: f64,
    pub(super) direction_sign: f64,
    pub(super) phase: f64,
    pub(super) last_update: Instant,
}

impl ToolVisual {
    pub(super) fn new() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            rpm: 0.0,
            max_rpm: 12000.0,
            direction_sign: 1.0,
            phase: 0.0,
            last_update: Instant::now(),
        }
    }
}

#[derive(Clone)]
pub(super) struct Camera {
    pub(super) orientation: CameraOrientation,
    pub(super) zoom: f64,
    pub(super) pan_x: f64,
    pub(super) pan_y: f64,
    pub(super) perspective: bool,
}
