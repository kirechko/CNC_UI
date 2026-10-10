use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct CNCStatus {
    #[allow(dead_code)]
    pub mode: String,
    pub state: String,
    pub x_position: f64,
    pub y_position: f64,
    pub z_position: f64,
    pub feed_rate: f64,
    pub spindle_speed: f64,
    pub spindle_on: bool,
    pub spindle_direction: SpindleDirection,
    pub jog_step: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpindleDirection {
    Off, CW, CCW,
}

pub struct CNCController {
    pub status: Arc<Mutex<CNCStatus>>,
}

impl CNCController {
    pub fn new() -> Self {
        let status = Arc::new(Mutex::new(CNCStatus {
            mode: "MANUAL".to_string(), state: "IDLE".to_string(),
            x_position: 0.0, y_position: 0.0, z_position: 0.0,
            feed_rate: 100.0, spindle_speed: 0.0, spindle_on: false,
            spindle_direction: SpindleDirection::Off, jog_step: 1.0,
        }));
        Self { status }
    }

    pub fn send_command(&self, command: &str) {
        println!("📤 CMD: {}", command);
    }

    pub fn set_spindle_speed(&self, speed: f64) {
        let mut status = self.status.lock().unwrap();
        if speed > 0.0 {
            status.spindle_speed = speed;
            let dir = match status.spindle_direction {
                SpindleDirection::Off | SpindleDirection::CW => SpindleDirection::CW,
                SpindleDirection::CCW => SpindleDirection::CCW,
            };
            let cmd = match dir {
                SpindleDirection::CW => format!("M3 S{:.0}", speed),
                SpindleDirection::CCW => format!("M4 S{:.0}", speed),
                SpindleDirection::Off => format!("M3 S{:.0}", speed),
            };
            drop(status);
            self.send_command(&cmd);
            let mut s = self.status.lock().unwrap();
            s.spindle_on = true;
            s.spindle_direction = dir;
        } else {
            status.spindle_speed = 0.0;
            status.spindle_on = false;
            status.spindle_direction = SpindleDirection::Off;
            drop(status);
            self.send_command("M5");
        }
    }

    pub fn set_spindle_direction(&self, direction: SpindleDirection) {
        let mut status = self.status.lock().unwrap();
        status.spindle_direction = direction.clone();
        let speed = status.spindle_speed;
        drop(status);
        match direction {
            SpindleDirection::CW if speed > 0.0 => self.send_command(&format!("M3 S{:.0}", speed)),
            SpindleDirection::CCW if speed > 0.0 => self.send_command(&format!("M4 S{:.0}", speed)),
            SpindleDirection::Off => {
                self.send_command("M5");
                let mut s = self.status.lock().unwrap();
                s.spindle_on = false;
            }
            _ => {}
        }
    }

    pub fn stop_spindle(&self) {
        let mut status = self.status.lock().unwrap();
        status.spindle_on = false;
        status.spindle_speed = 0.0;
        status.spindle_direction = SpindleDirection::Off;
        drop(status);
        self.send_command("M5");
    }

    pub fn set_jog_step(&self, step: f64) {
        let mut status = self.status.lock().unwrap();
        status.jog_step = step;
    }

    // Джог с явным указанием шага (для Jog Dial)
    pub fn jog_axis_step(&self, axis: char, direction: i8, step: f64) {
        let dist = step * direction as f64;
        self.send_command(&format!("G91 G0 {}{:.3}", axis, dist));
        self.send_command("G90");
    }

    // Джог с шагом из состояния (для кнопок Z)
    pub fn jog_axis(&self, axis: char, direction: i8) {
        let status = self.status.lock().unwrap();
        let step = status.jog_step;
        drop(status);
        self.jog_axis_step(axis, direction, step);
    }

    pub fn get_status_snapshot(&self) -> CNCStatus {
        self.status.lock().unwrap().clone()
    }
}
