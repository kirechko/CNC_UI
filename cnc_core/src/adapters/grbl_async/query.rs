//! Realtime status (`?`) and settings (`$$`) queries shared by the trait
//! facade and the streaming scheduler.

use super::{GrblAdapter, Session};
use crate::{ControllerError, CncResult};
use std::collections::HashMap;
use tokio::io::AsyncWriteExt;
use tokio::time::timeout;

impl GrblAdapter {
    /// Sends the realtime `?` request and returns the `<...>` status report
    /// line. Unlike `send_command`, a timeout here does not poison the
    /// protocol: a lost status reply does not desynchronize the ACK queue.
    pub(super) async fn query_status(session: &mut Session) -> CncResult<String> {
        Self::ensure_protocol_usable(session.protocol_poisoned)?;
        if let Err(error) = session.port.write_all(b"?").await {
            session.protocol_poisoned = true;
            return Err(ControllerError::new("io", error.to_string()));
        }

        match timeout(super::STATUS_TIMEOUT, async {
            loop {
                let line = match Self::read_line(
                    &mut session.port,
                    &mut session.rx_buffer,
                    super::STATUS_TIMEOUT,
                )
                .await
                {
                    Ok(line) => line,
                    Err(error) => {
                        session.protocol_poisoned = true;
                        return Err(error);
                    }
                };
                if line.starts_with('<') && line.ends_with('>') {
                    return Ok(line);
                }
            }
        })
        .await
        {
            Ok(result) => result,
            // A lost response to the realtime `?` request does not desynchronize the command ACK queue.
            // Do not block further requests and commands because of a single timeout.
            Err(_) => Err(ControllerError::new(
                "timeout",
                "timed out waiting for GRBL status",
            )),
        }
    }

    /// Runs `$$` and collects every `$N=value` setting until the final `ok`.
    /// A timeout poisons the protocol because the settings reply is a normal
    /// command transaction.
    pub(super) async fn query_settings(session: &mut Session) -> CncResult<HashMap<u16, f64>> {
        Self::ensure_protocol_usable(session.protocol_poisoned)?;
        if let Err(error) = session.port.write_all(b"$$\n").await {
            session.protocol_poisoned = true;
            return Err(ControllerError::new("io", error.to_string()));
        }

        match timeout(super::RESPONSE_TIMEOUT, async {
            let mut settings = HashMap::new();

            loop {
                let line = match Self::read_line(
                    &mut session.port,
                    &mut session.rx_buffer,
                    super::RESPONSE_TIMEOUT,
                )
                .await
                {
                    Ok(line) => line,
                    Err(error) => {
                        session.protocol_poisoned = true;
                        return Err(error);
                    }
                };
                if line == "ok" {
                    return Ok(settings);
                }
                if let Some(error) = line.strip_prefix("error:") {
                    return Err(ControllerError::new("settings", error));
                }
                if line.starts_with("ALARM:") {
                    return Err(ControllerError::new("alarm", line));
                }

                let Some(setting) = line.strip_prefix('$') else {
                    continue;
                };
                let Some((number, value)) = setting.split_once('=') else {
                    continue;
                };
                let (Ok(number), Ok(value)) = (number.parse::<u16>(), value.parse::<f64>()) else {
                    continue;
                };
                if value.is_finite() {
                    settings.insert(number, value);
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => {
                session.protocol_poisoned = true;
                Err(ControllerError::new(
                    "timeout",
                    "timed out waiting for GRBL settings; reconnect required",
                ))
            }
        }
    }
}
