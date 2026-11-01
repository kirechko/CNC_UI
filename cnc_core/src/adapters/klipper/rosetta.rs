//! Execution bridge for Rosetta macro G-code streaming over Moonraker.

use super::KlipperAdapter;
use crate::{CncResult, ControllerError};
use serde_json::json;

/// Streams one G-code block and waits for Moonraker's `notify_gcode_response` acknowledgement.
impl KlipperAdapter {
    /// Sends one script and processes notifications until its JSON-RPC acknowledgement arrives.
    pub async fn stream_gcode_block(
        &self,
        command: &str,
        cancellation: &std::sync::atomic::AtomicBool,
    ) -> CncResult<()> {
        if command.trim().is_empty() {
            return Ok(());
        }
        if cancellation.load(std::sync::atomic::Ordering::Acquire) {
            return Err(ControllerError::new(
                "command_cancelled",
                "Rosetta macro execution cancelled",
            ));
        }
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.rpc(
            &mut session,
            "printer.gcode.script",
            json!({"script": command}),
        )
        .await?;
        session.script_line = session.script_line.saturating_add(1);
        Ok(())
    }

    /// Resets the macro execution line counter before Rosetta streams a new run.
    pub async fn begin_rosetta_run(&self) {
        self.session.lock().await.script_line = 0;
    }

    /// Returns the current best-effort line index for Rosetta status observers.
    pub async fn rosetta_line(&self) -> u32 {
        self.session.lock().await.script_line
    }
}
