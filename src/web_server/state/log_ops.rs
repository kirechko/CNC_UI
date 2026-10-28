//! Message-log and command-history operations of [`AppState`], backed by the
//! single SQLite connection owned by the state (see `super::log_store` for
//! the raw SQL).

use super::AppState;
use crate::cnc_controller::ControllerSnapshot;
use crate::web_server::log_store;
use crate::web_server::models::{LatestLogMessages, LogPage, LogQuery};

impl AppState {
    /// Records one message into the SQLite log; informational and unknown
    /// severities are ignored.
    pub(crate) fn insert_log(&self, severity: &str, source: &str, message: &str) {
        if !crate::message_log::is_recorded_severity(severity) {
            return;
        }
        if let Ok(connection) = self.log.lock() {
            log_store::insert_message(&connection, severity, source, message);
        }
    }

    /// Logs an alert whenever the controller state/alert combination changes;
    /// identical successive snapshots are suppressed.
    pub(crate) fn record_status(&self, status: &ControllerSnapshot) {
        let alert = status
            .ui_alert
            .as_ref()
            .map(|(kind, message)| (kind, message));
        let key = format!(
            "{}\u{1f}{}\u{1f}{}",
            crate::web_server::models::status_state(status),
            alert.map_or("", |(kind, _)| kind.as_str()),
            alert.map_or("", |(_, message)| message.as_str()),
        );

        let Ok(mut previous) = self.last_logged_status.lock() else {
            return;
        };
        if previous.as_deref() == Some(&key) {
            return;
        }
        *previous = Some(key);
        drop(previous);

        if let Some((kind, message)) = alert {
            // Map controller state and alert kind to an internal severity identifier.
            let severity = match (crate::web_server::models::status_state(status), kind.as_str()) {
                ("ALARM", _) => crate::message_log::SEVERITY_ALARM,
                (_, "axis_limit") => crate::message_log::SEVERITY_WARNING,
                _ => crate::message_log::SEVERITY_ERROR,
            };
            self.insert_log(severity, "controller", message);
        }
    }

    /// Newest alarm plus newest non-alarm records for the header status chip.
    pub(crate) fn latest_log_messages(&self) -> Result<LatestLogMessages, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "message log lock poisoned".to_string())?;
        log_store::latest_messages(&connection)
    }

    /// Filtered, sorted, paginated view over the message log.
    pub(crate) fn query_log(&self, query: &LogQuery) -> Result<LogPage, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "message log lock poisoned".to_string())?;
        log_store::query(&connection, query)
    }

    pub(crate) fn command_history(&self) -> Result<Vec<String>, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "message log lock poisoned".to_string())?;
        log_store::command_history(&connection)
    }

    /// Endpoints confirmed by previous scans; the network scan probes them
    /// first so a restart does not lose the recently seen machines.
    pub(crate) fn cached_discovery_endpoints(&self) -> Result<Vec<String>, String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "message log lock poisoned".to_string())?;
        log_store::cached_endpoints(&connection)
    }

    /// Persists one freshly confirmed endpoint into the priority cache.
    pub(crate) fn cache_discovered_endpoint(&self, endpoint: &str) -> Result<(), String> {
        let connection = self
            .log
            .lock()
            .map_err(|_| "message log lock poisoned".to_string())?;
        log_store::cache_endpoint(&connection, endpoint)
    }

    /// Runs one console command: records it into the history (deduplicated)
    /// and forwards it to the controller worker.
    pub(crate) fn execute_command(&self, command: &str) -> Result<String, String> {
        let command = command.trim();
        if command.is_empty()
            || command.len() > 4096
            || command.contains('\n')
            || command.contains('\r')
        {
            return Err("Invalid command: provide one line of at most 4096 characters".into());
        }

        if let Ok(connection) = self.log.lock() {
            // `record_command` takes a plain reference; the mutex guard
            // derefs to `&Connection` automatically only for method calls.
            log_store::record_command(&connection, command);
        }

        let controller = self
            .controller
            .lock()
            .map_err(|_| "controller lock poisoned".to_string())?;
        controller
            .execute_command(command)
            .map_err(|error| format!("{}: {}", error.code, error.message))
    }
}
