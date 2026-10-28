//! Controller query helpers of [`FluidNcWebSocketAdapter`]: realtime status
//! polling, `$$` settings dumps, single `$ESP` capability reads and the
//! machine-configuration assembly built on top of them. The `CncApi` trait
//! implementation lives in `api.rs`.

use super::{FluidNcWebSocketAdapter, Session, STATUS_TIMEOUT_LIMIT};
use crate::discovery::{DiscoveredDevice, DiscoveryTransport};
use crate::{
    AxisConfiguration, AxisUnits, ControllerError, CncResult, MachineAxis, MachineConfiguration,
    SpindleConfiguration,
};
use std::{collections::HashMap, sync::atomic::Ordering, time::Instant};
use tokio::time::timeout;

impl FluidNcWebSocketAdapter {
    /// Ask the controller for one status report (`?`) and keep feeding the
    /// program window while unrelated acknowledgement lines arrive. Transient
    /// timeouts are tolerated up to `STATUS_TIMEOUT_LIMIT` consecutive misses
    /// before the session is dropped.
    pub(super) async fn query_status(&self, session: &mut Session) -> CncResult<String> {
        let started = Instant::now();
        let buffered_before = session.rx_buffer.len();
        self.send_realtime(session, b'?').await?;
        let result = match timeout(self.config.status_timeout, async {
            loop {
                let line = Self::read_line(session, self.config.status_timeout).await?;
                if line.starts_with('<') && line.ends_with('>') {
                    tracing::debug!(
                        elapsed_ms = started.elapsed().as_millis(),
                        buffered_before,
                        buffered_after = session.rx_buffer.len(),
                        response = %line,
                        "FluidNC status response received"
                    );
                    return Ok(line);
                }
                Self::record_program_response(session, &line);

                // Keep the command queue filled while the controller acknowledges lines
                // and the program still has commands to send.
                if session.program_streaming {
                    self.fill_program_window(session).await?;
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ControllerError::new(
                "timeout",
                "timed out waiting for FluidNC status",
            )),
        };

        if let Err(error) = &result {
            if std::env::var_os("CNC_UI_FLUIDNC_WS_TRACE").is_some() {
                eprintln!(
                    "[FluidNC WS] status query failed after {}ms (buffered_before={}): {}: {}; buffered={:?}",
                    started.elapsed().as_millis(),
                    buffered_before,
                    error.code,
                    error.message,
                    String::from_utf8_lossy(&session.rx_buffer),
                );
            }
            // A failed status query leaves unread/stale frames on the stream;
            // discard it so the next cycle performs fresh discovery/connect.
            // A single missed status reply is usually transient (busy controller,
            // delayed frame). Tolerate a bounded number of consecutive timeouts so
            // one late reply does not drop an active WebSocket session.
            let consecutive = self.status_timeouts.fetch_add(1, Ordering::SeqCst) + 1;
            if error.code == "timeout" && consecutive < STATUS_TIMEOUT_LIMIT {
                if std::env::var_os("CNC_UI_FLUIDNC_WS_TRACE").is_some() {
                    eprintln!(
                        "[FluidNC WS] status timeout tolerated consecutive={consecutive} limit={STATUS_TIMEOUT_LIMIT}"
                    );
                }
                tracing::warn!(
                    consecutive,
                    limit = STATUS_TIMEOUT_LIMIT,
                    elapsed_ms = started.elapsed().as_millis(),
                    buffered_before,
                    "FluidNC status timeout tolerated; keeping WebSocket session"
                );
            } else {
                self.status_timeouts.store(0, Ordering::SeqCst);
                if std::env::var_os("CNC_UI_FLUIDNC_WS_TRACE").is_some() {
                    eprintln!(
                        "[FluidNC WS] invalidating after status failure code={} consecutive={consecutive}",
                        error.code
                    );
                }
                tracing::warn!(
                    error_code = %error.code,
                    error = %error.message,
                    consecutive,
                    elapsed_ms = started.elapsed().as_millis(),
                    buffered_before,
                    buffered_after = session.rx_buffer.len(),
                    "invalidating FluidNC connection after status-query failure"
                );
                self.invalidate_connection(session);
            }
        }
        result
    }

    /// Sends one `$ESP` query and returns the value from its `$ESPnnn=value` line.
    async fn query_esp_setting(
        &self,
        session: &mut Session,
        command: &str,
    ) -> CncResult<Option<String>> {
        self.send_text(session, command).await?;
        let prefix = format!("{command}=");
        let mut value: Option<String> = None;
        let result = match timeout(self.config.ack_timeout, async {
            loop {
                let line = Self::read_line(session, self.config.ack_timeout).await?;
                if line == "ok" {
                    return Ok(());
                }
                if let Some(rest) = line.strip_prefix(&prefix) {
                    value = Some(rest.trim().to_string());
                }
                if let Some(error) = line.strip_prefix("error:") {
                    return Err(ControllerError::new(
                        "controller",
                        format!("FluidNC rejected {command} ({error})"),
                    ));
                }
            }
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(ControllerError::new(
                "timeout",
                "timed out waiting for FluidNC capability response",
            )),
        };
        result.map(|()| value)
    }

    /// Reads the other protocol settings of the connected FluidNC (`$ESP130/131`
    /// for Telnet, `$ESP120/121` for HTTP/WebSocket) instead of scanning the same
    /// host. Only values the controller actually reported become verified entries.
    pub(super) async fn query_protocol_capabilities(
        &self,
        session: &mut Session,
        address: std::net::Ipv4Addr,
    ) {
        let mut values: HashMap<&'static str, String> = HashMap::new();
        for command in ["$ESP130", "$ESP131", "$ESP120", "$ESP121"] {
            match self.query_esp_setting(session, command).await {
                Ok(Some(value)) => {
                    tracing::info!(command, value = %value, "FluidNC capability read");
                    values.insert(command, value);
                }
                Ok(None) => {
                    tracing::warn!(command, "FluidNC capability response had no value");
                }
                Err(error) => {
                    tracing::warn!(command, ?error, "FluidNC capability query failed");
                    return;
                }
            }
        }

        let mut verified = Vec::new();
        let telnet_enabled = values.get("$ESP130").is_some_and(|value| value == "ON");
        let telnet_port = values
            .get("$ESP131")
            .and_then(|value| value.parse::<u16>().ok());
        if let (true, Some(port)) = (telnet_enabled, telnet_port) {
            verified.push(DiscoveredDevice {
                id: format!("fluidnc:Telnet:{address}:{port}"),
                protocol: "FluidNC".into(),
                machine_name: None,
                version: None,
                transport: DiscoveryTransport::Telnet,
                endpoint: format!("{address}:{port}"),
            });
        }
        // FluidNC serves HTTP and WebSocket on the same port, so $ESP120/$ESP121
        // verify the WebSocket entry as well.
        let http_enabled = values.get("$ESP120").is_some_and(|value| value == "ON");
        let http_port = values
            .get("$ESP121")
            .and_then(|value| value.parse::<u16>().ok());
        if let (true, Some(port)) = (http_enabled, http_port) {
            verified.push(DiscoveredDevice {
                id: format!("fluidnc:WebSocket:{address}:{port}"),
                protocol: "FluidNC".into(),
                machine_name: None,
                version: None,
                transport: DiscoveryTransport::WebSocket,
                endpoint: format!("{address}:{port}"),
            });
        }
        let host = address.to_string();
        if let Ok(mut table) = self.verified_capabilities.write() {
            table.retain(|device| {
                device
                    .endpoint
                    .rsplit_once(':')
                    .is_none_or(|(device_host, _)| device_host != host)
            });
            table.extend(verified);
        }
    }

    /// Dump all `$N=value` GRBL-compatible settings into a number/value map.
    async fn query_settings(&self, session: &mut Session) -> CncResult<HashMap<u16, f64>> {
        self.send_text(session, "$$").await?;

        let result = match timeout(self.config.ack_timeout, async {
            let mut settings = HashMap::new();
            loop {
                let line = Self::read_line(session, self.config.ack_timeout).await?;

                if line == "ok" {
                    return Ok(settings);
                }
                if let Some(error) = line.strip_prefix("error:") {
                    return Err(ControllerError::new("settings", error));
                }
                if line.starts_with("ALARM:") {
                    return Err(ControllerError::new("alarm", line));
                }

                let Some((number, value)) = line
                    .strip_prefix('$')
                    .and_then(|setting| setting.split_once('='))
                else {
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
            Err(_) => Err(ControllerError::new(
                "timeout",
                "timed out waiting for FluidNC settings",
            )),
        };

        if result
            .as_ref()
            .is_err_and(|error| error.code != "settings" && error.code != "alarm")
        {
            self.invalidate_connection(session);
        }
        result
    }

    /// Assemble the machine configuration from one status poll (axis count) and
    /// a `$$` dump (travels, velocities, accelerations, steps and spindle RPM).
    /// Body of the `CncApi::machine_configuration` trait method.
    pub(super) async fn machine_configuration_snapshot(
        &self,
        session: &mut Session,
    ) -> CncResult<MachineConfiguration> {
        let status = self.query_status(session).await?;
        if let Some(count) = Self::reported_axis_count(&status) {
            session.reported_axis_count = count;
        }

        // FluidNC exposes GRBL-compatible travel and spindle limits through $$:
        // $130-$135 are the maximum travels for X/Y/Z/A/B/C, $30/$31 spindle RPM.
        let settings = self.query_settings(session).await?;
        let axis_definitions = [
            (MachineAxis::X, AxisUnits::Linear),
            (MachineAxis::Y, AxisUnits::Linear),
            (MachineAxis::Z, AxisUnits::Linear),
            (MachineAxis::A, AxisUnits::Angular),
            (MachineAxis::B, AxisUnits::Angular),
            (MachineAxis::C, AxisUnits::Angular),
        ];
        let axes = axis_definitions
            .into_iter()
            .enumerate()
            .take(session.reported_axis_count)
            .map(|(index, (axis, units))| AxisConfiguration {
                axis,
                units,
                max_travel: settings
                    .get(&(130 + index as u16))
                    .copied()
                    .filter(|value| *value > 0.0),
                max_velocity: settings
                    .get(&(110 + index as u16))
                    .copied()
                    .filter(|value| *value > 0.0),
                acceleration: settings
                    .get(&(120 + index as u16))
                    .copied()
                    .filter(|value| *value > 0.0),
                steps_per_unit: settings
                    .get(&(100 + index as u16))
                    .copied()
                    .filter(|value| *value > 0.0),
            })
            .collect();

        let min_rpm = settings.get(&31).copied().filter(|value| *value >= 0.0);
        let max_rpm = settings
            .get(&30)
            .copied()
            .filter(|value| *value > 0.0)
            .filter(|maximum| min_rpm.is_none_or(|minimum| minimum <= *maximum));

        Ok(MachineConfiguration {
            axes,
            spindle: SpindleConfiguration { min_rpm, max_rpm },
            settings,
        })
    }
}
