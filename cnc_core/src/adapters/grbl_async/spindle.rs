//! Spindle command normalization and RPM-limit validation.
//!
//! GRBL understands M3/M4/M5 with an S word; the helpers below translate the
//! neutral [`SpindleCommand`] into that vocabulary and refuse requests outside
//! the limits announced by the controller settings (`$$`).

use super::GrblAdapter;
use crate::{ControllerError, CncResult, SpindleCommand, SpindleDirection};

impl GrblAdapter {
    /// Checks the requested RPM against the configured spindle range.
    /// Stopping (direction `Stopped`) is always allowed.
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
                    "requested {:.0} RPM is outside the configured range {:.0}–{:.0} RPM",
                    command.rpm,
                    min_rpm.unwrap_or(0.0),
                    max_rpm.unwrap_or(f64::INFINITY),
                ),
            ));
        }

        Ok(())
    }

    /// A zero RPM or a stopped direction collapses to a single canonical
    /// "spindle off" command so later comparisons stay meaningful.
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
