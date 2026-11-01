//! The single `CncApi` implementation for [`KlipperAdapter`].

use super::{disconnected_status, KlipperAdapter, KlipperConfig, Session};
use crate::discovery::DiscoveredDevice;
use crate::{
    Axis, CncApi, CncEvent, CncResult, ControllerError, CoordinateSystem, JogRequest,
    MachineConfiguration, MachineStatus, OverrideKind, ProgramSource, SpindleCommand,
    SpindleDirection,
};
use serde_json::json;
use std::path::Path;
use tokio::sync::{broadcast, watch};

impl KlipperAdapter {
    /// Creates a lazily connected Moonraker adapter for the provided settings.
    pub fn connect(mut config: KlipperConfig) -> CncResult<Self> {
        let endpoint = config.endpoint.trim_end_matches('/').to_owned();
        config.endpoint = endpoint.clone();
        let (host, _) = endpoint
            .strip_prefix("http://")
            .or_else(|| endpoint.strip_prefix("https://"))
            .and_then(|value| value.split_once('/').or(Some((value, ""))))
            .unwrap_or((&endpoint, ""));
        let host = host.split(':').next().unwrap_or("moonraker");
        let device_id = config
            .machine_id
            .clone()
            .unwrap_or_else(|| format!("klipper:{host}"));
        let device = DiscoveredDevice {
            id: device_id,
            protocol: "Klipper".into(),
            machine_name: Some(host.to_owned()),
            version: None,
            transport: crate::discovery::DiscoveryTransport::WebSocket,
            endpoint,
        };
        let (events, _) = broadcast::channel(256);
        let (status_tx, _) = watch::channel(disconnected_status());
        Ok(Self {
            config,
            device,
            session: std::sync::Arc::new(tokio::sync::Mutex::new(Session {
                socket: None,
                next_id: 1,
                objects: serde_json::Map::new(),
                object_names: Vec::new(),
                machine_name: None,
                config_settings: serde_json::Value::Null,
                laser_pin: None,
                spindle_command: SpindleCommand {
                    direction: SpindleDirection::Stopped,
                    rpm: 0.0,
                },
                program_name: None,
                toolpath: crate::Toolpath::default(),
                script_line: 0,
                total_program_lines: 0,
                continuous_jog: None,
                subscribed: false,
                estopped: false,
            })),
            status_tx,
            events,
            last_error: std::sync::Arc::new(tokio::sync::Mutex::new(None)),
        })
    }

    /// Constructs a Klipper endpoint with default request timeouts and no API key.
    pub fn connect_url(endpoint: impl Into<String>) -> CncResult<Self> {
        Self::connect(KlipperConfig::new(endpoint))
    }

    /// Returns the most recent known controller snapshot.
    pub fn last_status_snapshot(&self) -> MachineStatus {
        self.status_tx.borrow().clone()
    }

    /// Loads a local program to Moonraker's `gcodes` root using multipart REST upload.
    async fn upload_program(&self, path: &Path) -> CncResult<()> {
        let data = tokio::fs::read(path)
            .await
            .map_err(|e| ControllerError::new("program_read", e.to_string()))?;
        let filename = path
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("program.gcode");
        let boundary = "CNCUIKlipperUploadBoundary";
        let mut body = Vec::new();
        let relative_path = filename;
        let (directory, basename) = relative_path
            .rsplit_once('/')
            .unwrap_or(("", relative_path));
        let root_field = if directory.is_empty() {
            "gcodes".to_owned()
        } else {
            format!("gcodes/{directory}")
        };
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"root\"\r\n\r\n{root_field}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{basename}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        body.extend_from_slice(&data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        rest_post(
            &self.config,
            "/server/files/upload",
            &format!("multipart/form-data; boundary={boundary}"),
            &body,
        )
        .await?;
        let mut session = self.session.lock().await;
        session.program_name = Some(filename.to_owned());
        session.script_line = 0;
        session.total_program_lines = String::from_utf8_lossy(&data)
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count() as u32;
        session.toolpath = crate::parse_toolpath(&String::from_utf8_lossy(&data));
        Ok(())
    }
    /// Returns the last controller error for log/console consumers.
    pub async fn take_last_error(&self) -> Option<String> {
        self.last_error.lock().await.take()
    }
}

#[async_trait::async_trait]
impl CncApi for KlipperAdapter {
    fn klipper_adapter(&self) -> Option<&KlipperAdapter> {
        Some(self)
    }
    /// Queries Moonraker and returns the latest printer object status.
    async fn status(&self) -> CncResult<MachineStatus> {
        let mut session = self.session.lock().await;
        if let Err(error) = self.ensure_connected(&mut session).await {
            if Self::laser_watchdog_configured(&session) {
                tracing::warn!("Klipper laser watchdog configured for connection loss");
            } else if session.laser_pin.is_some() {
                tracing::warn!("Klipper laser output lacks maximum_mcu_duration; connection-loss safety is not guaranteed");
            }
            let disconnected = disconnected_status();
            self.publish_status(disconnected.clone());
            return if matches!(
                error.code.as_str(),
                "timeout" | "connection" | "transport" | "disconnected"
            ) {
                Ok(disconnected)
            } else {
                Err(error)
            };
        }
        match self.refresh_status(&mut session).await {
            Ok(status) => Ok(status),
            Err(error)
                if matches!(
                    error.code.as_str(),
                    "timeout" | "connection" | "transport" | "disconnected"
                ) =>
            {
                session.socket = None;
                session.subscribed = false;
                let disconnected = disconnected_status();
                self.publish_status(disconnected.clone());
                Ok(disconnected)
            }
            Err(error) => Err(error),
        }
    }

    /// Returns this adapter's stable endpoint identifier.
    fn connected_device_id(&self) -> Option<String> {
        Some(self.device.id.clone())
    }

    /// Returns the latest cached status without network I/O.
    fn last_status(&self) -> Option<MachineStatus> {
        Some(self.status_tx.borrow().clone())
    }

    /// Subscribes to status updates.
    fn subscribe_status(&self) -> Option<watch::Receiver<MachineStatus>> {
        Some(self.status_tx.subscribe())
    }

    /// Subscribes to lifecycle and controller error events.
    fn subscribe_events(&self) -> Option<broadcast::Receiver<CncEvent>> {
        Some(self.events.subscribe())
    }

    /// Gets the configuration parsed by Moonraker from printer.cfg.
    async fn machine_configuration(&self) -> CncResult<MachineConfiguration> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        Ok(super::status::configuration_snapshot(&session))
    }

    /// Acknowledges controller readiness without toggling machine power.
    async fn power_on(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let state = self.rpc(&mut session, "server.info", json!({})).await?;
        if state["klippy_state"].as_str() == Some("shutdown") {
            self.rpc(&mut session, "printer.firmware_restart", json!({}))
                .await?;
        }
        Ok(())
    }

    /// Klipper has no general-purpose software power-off operation.
    async fn power_off(&self) -> CncResult<()> {
        Err(ControllerError::new(
            "unsupported",
            "Klipper has no software power-off command",
        ))
    }

    /// Sends Moonraker emergency stop and immediately drops laser output if possible.
    async fn estop(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let result = self.emergency_stop_and_disable_laser(&mut session).await;
        session.socket = None;
        session.subscribed = false;
        let mut status = disconnected_status();
        status.message = Some(
            "Klipper is in shutdown after emergency stop; use firmware restart to recover".into(),
        );
        self.publish_status(status);
        result
    }

    /// Homes all axes or the requested axis.
    async fn home(&self, axis: Option<Axis>) -> CncResult<()> {
        let script = axis.map_or_else(
            || "G28".to_owned(),
            |a| format!("G28 {}", Self::axis_letter(a)),
        );
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.script(&mut session, &script).await
    }

    /// Klipper exposes one work coordinate system; other UI WCS selections are rejected.
    async fn select_coordinate_system(&self, system: CoordinateSystem) -> CncResult<()> {
        if system != CoordinateSystem::G54 {
            return Err(ControllerError::new(
                "unsupported",
                "Klipper exposes one work coordinate system (G54)",
            ));
        }
        Ok(())
    }

    /// Sets all work coordinates to zero using G92.
    async fn set_work_zero(&self, system: CoordinateSystem) -> CncResult<()> {
        if system != CoordinateSystem::G54 {
            return Err(ControllerError::new(
                "unsupported",
                "Klipper exposes one work coordinate system (G54)",
            ));
        }
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.script(&mut session, "G92 X0 Y0 Z0").await
    }

    /// Sets one work coordinate to zero using G92.
    async fn set_axis_work_zero(&self, axis: Axis) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.script(&mut session, &format!("G92 {}0", Self::axis_letter(axis)))
            .await
    }

    /// Performs one safe incremental jog, restoring absolute mode in the same script.
    async fn jog_increment(&self, request: JogRequest) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.incremental_jog(&mut session, request).await
    }

    /// Starts emulated continuous jogging by issuing its first short segment.
    async fn jog_continuous(&self, axis: Axis, direction: i8, feed_rate: f64) -> CncResult<()> {
        if ![-1, 0, 1].contains(&direction) {
            return Err(ControllerError::new("bad_jog", "invalid jog direction"));
        }
        if direction == 0 {
            return self.cancel_jog().await;
        }
        if !feed_rate.is_finite() || feed_rate <= 0.0 {
            return Err(ControllerError::new(
                "bad_jog",
                "jog feed rate must be positive",
            ));
        }
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        session.continuous_jog = Some((axis, direction, feed_rate));
        self.send_next_jog_chunk(&mut session).await
    }

    /// Stops feeding new continuous-jog chunks; at most one already queued segment remains.
    async fn cancel_jog(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        session.continuous_jog = None;
        if session.socket.is_some() {
            self.script(&mut session, "M400").await?;
        }
        Ok(())
    }

    /// Executes a console G-code script.
    async fn mdi(&self, command: &str) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.script(&mut session, command).await
    }

    /// Loads a host G-code file to Moonraker's `gcodes` folder.
    async fn load_program(&self, path: &Path) -> CncResult<()> {
        self.upload_program(path).await
    }

    /// Uploads a local file or accepts an inline program, then starts printing when requested separately.
    async fn load_program_source(&self, source: ProgramSource) -> CncResult<()> {
        match source {
            ProgramSource::Path(path) => self.upload_program(&path).await,
            ProgramSource::Inline(content) => {
                let filename = "inline.gcode";
                self.upload_bytes("gcodes", filename, content.as_bytes())
                    .await?;
                let mut session = self.session.lock().await;
                session.program_name = Some(filename.to_owned());
                session.toolpath = crate::parse_toolpath(&content);
                session.total_program_lines = content
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .count() as u32;
                session.script_line = 0;
                Ok(())
            }
        }
    }

    /// Starts the loaded file through Moonraker.
    async fn program_run(&self, _start_line: u32) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let filename = session
            .program_name
            .clone()
            .ok_or_else(|| ControllerError::new("bad_program", "no program has been uploaded"))?;
        self.rpc(
            &mut session,
            "printer.print.start",
            json!({"filename": filename}),
        )
        .await
        .map(|_| ())
    }

    /// Pauses print execution and disables laser output.
    async fn program_pause(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let result = self
            .rpc(&mut session, "printer.print.pause", json!({}))
            .await;
        let laser = self.enforce_laser_off(&mut session).await;
        result.map(|_| ()).and(laser)
    }

    /// Resumes the Moonraker print lifecycle.
    async fn program_resume(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.rpc(&mut session, "printer.print.resume", json!({}))
            .await
            .map(|_| ())
    }

    /// Cancels printing and disables laser output.
    async fn program_abort(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let result = self
            .rpc(&mut session, "printer.print.cancel", json!({}))
            .await;
        let laser = self.enforce_laser_off(&mut session).await;
        result.map(|_| ()).and(laser)
    }

    /// Sets the feed override via M220.
    async fn set_override(&self, kind: OverrideKind, value: f64) -> CncResult<()> {
        if kind != OverrideKind::Feed {
            return Err(ControllerError::new(
                "unsupported",
                "Klipper supports feed override only",
            ));
        }
        if !value.is_finite() || !(10.0..=200.0).contains(&value) {
            return Err(ControllerError::new(
                "bad_override",
                "feed override must be between 10 and 200 percent",
            ));
        }
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.script(&mut session, &format!("M220 S{value:.0}"))
            .await
    }

    /// Controls conventional spindle macros or the configured PWM output fallback.
    async fn set_spindle(&self, command: SpindleCommand) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        if session.laser_pin.is_some()
            || session.object_names.iter().any(|name| {
                name.eq_ignore_ascii_case("gcode_macro M3")
                    || name.eq_ignore_ascii_case("gcode_macro M4")
            })
        {
            return self.set_laser_output(&mut session, command).await;
        }
        self.spindle(&mut session, command).await
    }

    /// Stops an active Klipper shutdown using firmware_restart.
    async fn clear_error(&self) -> CncResult<()> {
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.rpc(&mut session, "printer.firmware_restart", json!({}))
            .await?;
        session.estopped = false;
        session.subscribed = false;
        Ok(())
    }
}

/// Sends a multipart upload to Moonraker's HTTP API using Tokio TCP without blocking the runtime.
pub(super) async fn rest_post(
    config: &KlipperConfig,
    path: &str,
    content_type: &str,
    body: &[u8],
) -> CncResult<()> {
    let endpoint = config.endpoint.strip_prefix("http://").ok_or_else(|| {
        ControllerError::new(
            "unsupported",
            "Moonraker REST upload currently requires http://",
        )
    })?;
    let (authority, base_path) = endpoint.split_once('/').unwrap_or((endpoint, ""));
    let address = if authority.contains(':') {
        authority.to_owned()
    } else {
        format!("{authority}:7125")
    };
    let target = format!("/{base_path}{path}");
    let mut stream = tokio::time::timeout(
        config.request_timeout,
        tokio::net::TcpStream::connect(&address),
    )
    .await
    .map_err(|_| ControllerError::new("timeout", "Moonraker upload connection timed out"))?
    .map_err(|e| ControllerError::new("transport", e.to_string()))?;
    let auth = config
        .api_key
        .as_ref()
        .map(|key| format!("X-Api-Key: {key}\r\n"))
        .unwrap_or_default();
    let header = format!("POST {target} HTTP/1.1\r\nHost: {authority}\r\n{auth}Content-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    stream
        .write_all(header.as_bytes())
        .await
        .map_err(|e| ControllerError::new("transport", e.to_string()))?;
    stream
        .write_all(body)
        .await
        .map_err(|e| ControllerError::new("transport", e.to_string()))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|e| ControllerError::new("transport", e.to_string()))?;
    let status = String::from_utf8_lossy(&response)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    if !status.contains(" 2") {
        return Err(ControllerError::new("moonraker_http", status));
    }
    Ok(())
}
