use super::Emulator;
use super::super::protocol::{api_error, WireError};
use crate::cnc_api::TaskState;

impl Emulator {
    pub(super) fn ensure_motion_allowed(&self) -> Result<(), WireError> {
        if self.status.task_state != TaskState::On {
            return Err(api_error("machine_off", "virtual machine is not powered on"));
        }
        Ok(())
    }
}
