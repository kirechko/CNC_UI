//! Application-state operations for persistent user-authored machine profiles.

use super::AppState;
use crate::web_server::machine_profiles::{self, MachineProfile, MachineProfileDocument};

impl AppState {
    /// Returns locally authored profiles in deterministic display order.
    pub(crate) fn list_machine_profiles(&self) -> Result<Vec<MachineProfile>, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "profile database lock poisoned".to_string())?;
        machine_profiles::list(&connection)
    }

    /// Creates or updates one profile without altering controller-reported machine configuration.
    pub(crate) fn save_machine_profile(&self, profile: &MachineProfile) -> Result<(), String> {
        let mut connection = self
            .log
            .lock()
            .map_err(|_| "profile database lock poisoned".to_string())?;
        machine_profiles::upsert(&mut connection, profile)
    }

    /// Removes one local profile and clears its durable active selection when the deleted id was selected.
    pub(crate) fn delete_machine_profile(&self, id: &str) -> Result<bool, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "profile database lock poisoned".to_string())?;
        let deleted = machine_profiles::delete(&connection, id)?;
        drop(connection);
        if deleted {
            let selected = self.active_machine_profile_id()?;
            *self
                .active_profile_id
                .lock()
                .map_err(|_| "active profile lock poisoned".to_string())? = selected;
        }
        Ok(deleted)
    }

    /// Imports a versioned profile document transactionally after validating every record.
    pub(crate) fn import_machine_profiles(
        &self,
        document: &MachineProfileDocument,
    ) -> Result<(), String> {
        let mut connection = self
            .log
            .lock()
            .map_err(|_| "profile database lock poisoned".to_string())?;
        machine_profiles::import_document(&mut connection, document)
    }

    /// Persists the local preflight profile choice without mutating controller settings.
    pub(crate) fn select_machine_profile(&self, id: Option<&str>) -> Result<(), String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "profile database lock poisoned".to_string())?;
        machine_profiles::select_profile(&connection, id)?;
        *self
            .active_profile_id
            .lock()
            .map_err(|_| "active profile lock poisoned".to_string())? = id.map(str::to_owned);
        Ok(())
    }

    /// Returns the durable selected profile id, refreshing the compatibility cache from SQLite.
    pub(crate) fn active_machine_profile_id(&self) -> Result<Option<String>, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "profile database lock poisoned".to_string())?;
        let selected = machine_profiles::active_profile_id(&connection)?;
        *self
            .active_profile_id
            .lock()
            .map_err(|_| "active profile lock poisoned".to_string())? = selected.clone();
        Ok(selected)
    }
}
