#[path = "gcode/accessories.rs"]
mod accessories;
#[path = "gcode/coordinates.rs"]
mod coordinates;
#[path = "gcode/execute.rs"]
mod execute;
#[path = "gcode/motion.rs"]
mod motion;
#[path = "gcode/parser.rs"]
mod parser;

#[cfg(test)]
#[path = "gcode/test_support.rs"]
pub(super) mod test_support;
#[cfg(test)]
pub(super) use test_support::test_emulator;

use super::protocol::WireError;
use super::state::Emulator;
use crate::cnc_api::*;

impl Emulator {
    pub(super) fn execute_gcode(&mut self, line: &str) -> Result<(), WireError> {
        execute::execute_gcode(self, line)
    }

    pub(super) fn set_spindle(&mut self, direction: SpindleDirection, rpm: Option<f64>) {
        accessories::set_spindle(self, direction, rpm);
    }
}
