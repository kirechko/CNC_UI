//! JSON-RPC 2.0 transport over Moonraker's WebSocket endpoint.

use super::{KlipperAdapter, Session};
use crate::{CncResult, ControllerError};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::{connect_async, tungstenite::Message};

impl KlipperAdapter {
    /// Opens the Moonraker socket, identifies this client and establishes object subscriptions.
    pub(super) async fn ensure_connected(&self, session: &mut Session) -> CncResult<()> {
        if session.estopped {
            return Err(ControllerError::new(
                "klippy_shutdown",
                "Klipper is shut down after E-stop; call firmware_restart to recover",
            ));
        }
        if session.socket.is_some() && session.subscribed {
            return Ok(());
        }
        if session.socket.is_none() {
            let mut url = websocket_url(&self.config.endpoint)?;
            if let Some(key) = self.config.api_key.as_deref() {
                let token = self.one_shot_token(key).await?;
                url.push_str(if url.contains('?') {
                    "&token="
                } else {
                    "?token="
                });
                url.push_str(&token);
            }
            let (socket, _) = tokio::time::timeout(self.config.request_timeout, connect_async(url))
                .await
                .map_err(|_| ControllerError::new("timeout", "Moonraker connection timed out"))?
                .map_err(|error| ControllerError::new("connection", error.to_string()))?;
            session.socket = Some(socket);
            session.subscribed = false;
        }
        let result = self.initialize_subscription(session).await;
        if result.is_err() {
            session.socket = None;
            session.subscribed = false;
        }
        result
    }

    /// Identifies the UI client and subscribes to available motion, print, config, macro and output objects.
    async fn initialize_subscription(&self, session: &mut Session) -> CncResult<()> {
        self.rpc_connected(session, "server.connection.identify", json!({
            "client_name": "CNC_UI", "version": env!("CARGO_PKG_VERSION"), "type": "http", "url": "https://github.com/CNC_UI"
        })).await?;
        let list = self
            .rpc_connected(session, "printer.objects.list", json!({}))
            .await?;
        session.object_names = list["objects"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let info = self
            .rpc_connected(session, "server.info", json!({}))
            .await?;
        session.machine_name = info["hostname"].as_str().map(str::to_owned);
        let requested = super::status::subscription_objects(&session.object_names)
            .into_iter()
            .map(|object| (object, Value::Null))
            .collect::<serde_json::Map<_, _>>();
        let initial = self
            .rpc_connected(
                session,
                "printer.objects.subscribe",
                json!({"objects": requested}),
            )
            .await?;
        session.objects.clear();
        if let Some(status) = initial.get("status").and_then(Value::as_object) {
            session.objects.extend(status.clone());
        }
        session.config_settings = session
            .objects
            .get("configfile")
            .and_then(|value| value.get("settings"))
            .cloned()
            .unwrap_or(Value::Null);
        session.laser_pin = super::capabilities::detect_laser_pin(&session.config_settings);
        session.subscribed = true;
        Ok(())
    }

    /// Ensures a connection exists before a normal RPC and invalidates it after transport failure.
    pub(super) async fn rpc(
        &self,
        session: &mut Session,
        method: &str,
        params: Value,
    ) -> CncResult<Value> {
        if session.socket.is_none() || !session.subscribed {
            self.ensure_connected(session).await?;
        }
        match self.rpc_connected(session, method, params).await {
            Ok(value) => Ok(value),
            Err(error) => {
                if matches!(
                    error.code.as_str(),
                    "transport" | "disconnected" | "timeout"
                ) {
                    session.socket = None;
                    session.subscribed = false;
                    self.publish_status(super::disconnected_status());
                    let _ = self.events.send(crate::CncEvent::Disconnected {
                        reason: error.message.clone(),
                    });
                }
                Err(error)
            }
        }
    }

    /// Exchanges a single correlated JSON-RPC request on an established socket.
    pub(super) async fn rpc_connected(
        &self,
        session: &mut Session,
        method: &str,
        params: Value,
    ) -> CncResult<Value> {
        let id = session.next_id;
        session.next_id = session.next_id.wrapping_add(1).max(1);
        let request = json!({"jsonrpc":"2.0", "method":method, "params":params, "id":id});
        session
            .socket
            .as_mut()
            .expect("socket initialized")
            .send(Message::Text(request.to_string()))
            .await
            .map_err(|error| ControllerError::new("transport", error.to_string()))?;
        tokio::time::timeout(self.config.request_timeout, async {
            loop {
                let message = session
                    .socket
                    .as_mut()
                    .expect("socket initialized")
                    .next()
                    .await
                    .ok_or_else(|| {
                        ControllerError::new("disconnected", "Moonraker WebSocket closed")
                    })?
                    .map_err(|error| ControllerError::new("transport", error.to_string()))?;
                match message {
                    Message::Text(text) => {
                        let value: Value = serde_json::from_str(&text)
                            .map_err(|error| ControllerError::new("protocol", error.to_string()))?;
                        if value.get("method").and_then(Value::as_str).is_some() {
                            let event = value.clone();
                            self.dispatch_notification(session, &event).await;
                            continue;
                        }
                        if value.get("id").and_then(Value::as_u64) == Some(id) {
                            if let Some(error) = value.get("error") {
                                return Err(ControllerError::new(
                                    "moonraker_rpc",
                                    error.to_string(),
                                ));
                            }
                            return Ok(value.get("result").cloned().unwrap_or(Value::Null));
                        }
                        // JSON-RPC notifications may have no id; unrelated responses are ignored.
                    }
                    Message::Ping(payload) => session
                        .socket
                        .as_mut()
                        .expect("socket initialized")
                        .send(Message::Pong(payload))
                        .await
                        .map_err(|error| ControllerError::new("transport", error.to_string()))?,
                    Message::Close(_) => {
                        return Err(ControllerError::new(
                            "disconnected",
                            "Moonraker WebSocket closed",
                        ))
                    }
                    _ => {}
                }
            }
        })
        .await
        .map_err(|_| {
            ControllerError::new("timeout", format!("Moonraker RPC `{method}` timed out"))
        })?
    }

    /// Dispatches status and Klippy lifecycle notifications without issuing nested RPC calls.
    async fn dispatch_notification(&self, session: &mut Session, message: &Value) {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let args = message
            .get("params")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        match method {
            "notify_status_update" => {
                if let Some(update) = args.first().and_then(Value::as_object) {
                    for (key, value) in update {
                        merge_object(&mut session.objects, key, value);
                    }
                    if let Ok(status) = super::status::map_status(session, true) {
                        self.publish_status(status);
                    }
                }
            }
            "notify_klippy_shutdown" => {
                let reason = args
                    .first()
                    .and_then(Value::as_str)
                    .unwrap_or("Klippy shutdown")
                    .to_owned();
                self.last_error.lock().await.replace(reason.clone());
                let _ = self.events.send(crate::CncEvent::ProtocolError {
                    code: "klippy_shutdown".into(),
                    message: reason,
                });
                if let Some(webhooks) = session
                    .objects
                    .get_mut("webhooks")
                    .and_then(Value::as_object_mut)
                {
                    webhooks.insert("state".into(), Value::String("shutdown".into()));
                }
                if let Ok(status) = super::status::map_status(session, true) {
                    self.publish_status(status);
                }
            }
            "notify_klippy_ready" => {
                session.estopped = false;
                session.objects.clear();
                session.config_settings = Value::Null;
                session.subscribed = false;
            }
            "notify_gcode_response" => {
                if let Some(line) = args.first().and_then(Value::as_str) {
                    if line.starts_with("!!") {
                        self.last_error.lock().await.replace(line.to_owned());
                        let _ = self.events.send(crate::CncEvent::ProtocolError {
                            code: "gcode_error".into(),
                            message: line.to_owned(),
                        });
                        session
                            .objects
                            .insert("__error".into(), Value::String(line.to_owned()));
                        if session.laser_pin.is_some() {
                            session.spindle_command = crate::SpindleCommand {
                                direction: crate::SpindleDirection::Stopped,
                                rpm: 0.0,
                            };
                        }
                        if let Ok(status) = super::status::map_status(session, true) {
                            self.publish_status(status);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Publishes a snapshot and reports coarse state transitions.
    pub(super) fn publish_status(&self, status: crate::MachineStatus) {
        let old = self.status_tx.borrow().clone();
        let from = crate::machine_state(&old);
        let to = crate::machine_state(&status);
        if from != to {
            let _ = self.events.send(crate::CncEvent::StateChanged { from, to });
        }
        self.status_tx.send_replace(status);
        let _ = self.events.send(crate::CncEvent::StatusUpdate);
    }

    /// Requests a Moonraker one-shot WebSocket token for an API key.
    async fn one_shot_token(&self, api_key: &str) -> CncResult<String> {
        let endpoint = self
            .config
            .endpoint
            .strip_prefix("http://")
            .ok_or_else(|| {
                ControllerError::new(
                    "unsupported",
                    "API-key authentication currently requires http:// Moonraker",
                )
            })?;
        let (authority, base_path) = endpoint.split_once('/').unwrap_or((endpoint, ""));
        let address = if authority.contains(':') {
            authority.to_owned()
        } else {
            format!("{authority}:7125")
        };
        let mut stream = tokio::time::timeout(
            self.config.request_timeout,
            tokio::net::TcpStream::connect(&address),
        )
        .await
        .map_err(|_| ControllerError::new("timeout", "Moonraker token request timed out"))?
        .map_err(|error| ControllerError::new("transport", error.to_string()))?;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let prefix = if base_path.is_empty() {
            String::new()
        } else {
            format!("/{}/", base_path.trim_matches('/'))
        };
        let request = format!("GET {prefix}access/oneshot_token HTTP/1.1\r\nHost: {authority}\r\nX-Api-Key: {api_key}\r\nConnection: close\r\n\r\n");
        stream
            .write_all(request.as_bytes())
            .await
            .map_err(|error| ControllerError::new("transport", error.to_string()))?;
        let mut response = Vec::new();
        tokio::time::timeout(
            self.config.request_timeout,
            stream.read_to_end(&mut response),
        )
        .await
        .map_err(|_| ControllerError::new("timeout", "Moonraker token response timed out"))?
        .map_err(|error| ControllerError::new("transport", error.to_string()))?;
        let text = String::from_utf8_lossy(&response);
        let body = text
            .split_once("\r\n\r\n")
            .map(|(_, body)| body)
            .ok_or_else(|| {
                ControllerError::new("protocol", "malformed Moonraker token response")
            })?;
        let value: Value = serde_json::from_str(body)
            .map_err(|error| ControllerError::new("protocol", error.to_string()))?;
        value
            .get("result")
            .and_then(|result| result.get("token"))
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| {
                ControllerError::new(
                    "authentication",
                    "Moonraker did not return a one-shot token",
                )
            })
    }
}

/// Converts an HTTP(S) Moonraker base URL to WebSocket form.
fn websocket_url(endpoint: &str) -> CncResult<String> {
    let endpoint = endpoint.trim_end_matches('/');
    if let Some(rest) = endpoint.strip_prefix("https://") {
        Ok(format!("wss://{rest}/websocket"))
    } else if let Some(rest) = endpoint.strip_prefix("http://") {
        Ok(format!("ws://{rest}/websocket"))
    } else {
        Err(ControllerError::new(
            "configuration",
            "Moonraker URL must use http:// or https://",
        ))
    }
}

/// Exposes URL conversion for deterministic unit tests.
#[cfg(test)]
pub(super) fn test_websocket_url(endpoint: &str) -> String {
    websocket_url(endpoint).expect("test endpoint must be valid")
}

/// Merges a partial object update while retaining unrelated fields.
fn merge_object(objects: &mut serde_json::Map<String, Value>, key: &str, delta: &Value) {
    match (objects.get_mut(key), delta) {
        (Some(Value::Object(current)), Value::Object(update)) => {
            for (field, value) in update {
                current.insert(field.clone(), value.clone());
            }
        }
        _ => {
            objects.insert(key.to_owned(), delta.clone());
        }
    }
}

/// Captures a Klipper `!!` response and clears the cached laser command after an execution fault.
#[cfg(test)]
pub(super) fn capture_gcode_error(session: &mut Session, line: &str) -> bool {
    if !line.starts_with("!!") {
        return false;
    }
    session
        .objects
        .insert("__error".into(), Value::String(line.to_owned()));
    if session.laser_pin.is_some() {
        session.spindle_command = crate::SpindleCommand {
            direction: crate::SpindleDirection::Stopped,
            rpm: 0.0,
        };
    }
    true
}
