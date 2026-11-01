//! Spindle/laser command normalization and native S-limit validation.
//!
//! GRBL understands M3/M4/M5 with an S word; in `$32=1` laser mode M4 selects
//! dynamic power and direction reversal is not a meaningful operation.

use super::GrblAdapter;
use crate::{CncResult, ControllerError, SpindleCommand, SpindleDirection};

impl GrblAdapter {
    /// Checks native S units against the configured output range; stopping is always allowed.
    pub(super) fn validate_spindle_command(
        command: SpindleCommand,
        min_rpm: Option<f64>,
        max_rpm: Option<f64>,
    ) -> CncResult<()> {
        if command.direction == SpindleDirection::Stopped {
            return Ok(());
        }

        if min_rpm.is_some_and(|minimum| command.rpm < minimum)
            || max_rpm.is_some_and(|maximum| command.rpm > maximum)
        {
            return Err(ControllerError::new(
                "spindle_limit",
                format!(
                    "requested S{:.0} is outside the configured range {:.0}–{:.0}",
                    command.rpm,
                    min_rpm.unwrap_or(0.0),
                    max_rpm.unwrap_or(f64::INFINITY),
                ),
            ));
        }

        Ok(())
    }

    /// A zero S value or stopped direction collapses to a canonical off command for spindle and laser modes.
    pub(super) fn normalize_spindle_command(command: SpindleCommand) -> SpindleCommand {
        if command.rpm == 0.0 || command.direction == SpindleDirection::Stopped {
            SpindleCommand {
                direction: SpindleDirection::Stopped,
                rpm: 0.0,
            }
        } else {
            command
        }
    }
}
