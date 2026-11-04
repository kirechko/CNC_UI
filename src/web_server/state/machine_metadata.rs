//! Read-only profile/controller metadata resolution operations for `AppState`.

use super::AppState;
use crate::web_server::machine_profiles::{self, ResolvedConfiguration};

impl AppState {
    /// Resolves selected profile metadata against the cached adapter snapshot without performing I/O to the machine.
    pub(crate) fn resolved_machine_configuration(
        &self,
        profile_id: Option<&str>,
    ) -> Result<ResolvedConfiguration, String> {
        let profiles = self.list_machine_profiles()?;
        let selected = match profile_id {
            Some(id) => Some(
                profiles
                    .into_iter()
                    .find(|profile| profile.id == id)
                    .ok_or_else(|| "selected machine profile does not exist".to_string())?,
            ),
            None => None,
        };
        let controller_configuration = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?
            .get_status_snapshot()
            .machine_configuration;
        let tools = self.list_tools()?;
        Ok(machine_profiles::resolution::resolve(
            selected.as_ref(),
            controller_configuration.as_ref(),
            &tools,
        ))
    }
}
