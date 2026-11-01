//! Moonraker file API helpers for the `gcodes` and `config` roots.

use crate::{CncResult, ControllerError};
use serde_json::Value;

/// A file or directory exposed by Moonraker's `gcodes` or `config` root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KlipperFileEntry {
    /// Name relative to the listed directory.
    pub name: String,
    /// Whether this entry is a directory.
    pub is_dir: bool,
    /// File size in bytes; directories report zero.
    pub size: u64,
}

impl super::KlipperAdapter {
    /// Lists one directory using Moonraker's `server.files.list` RPC method.
    pub async fn list_files(&self, root: &str, path: &str) -> CncResult<Vec<KlipperFileEntry>> {
        validate_root(root)?;
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        let response = self
            .rpc(
                &mut session,
                "server.files.list",
                serde_json::json!({"root": root, "path": path}),
            )
            .await?;
        Ok(response
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(parse_entry)
            .collect())
    }

    /// Uploads bytes to a root-relative path with multipart REST.
    pub async fn upload_bytes(&self, root: &str, filename: &str, data: &[u8]) -> CncResult<()> {
        validate_root(root)?;
        if filename.contains(['\\', '\r', '\n', '"'])
            || filename.split('/').any(|part| part == ".." || part == ".")
        {
            return Err(ControllerError::new("bad_path", "invalid upload path"));
        }
        let boundary = "CNCUIKlipperUploadBoundary";
        let mut body = Vec::new();
        let (directory, basename) = filename.rsplit_once('/').unwrap_or(("", filename));
        let root_field = if directory.is_empty() {
            root.to_owned()
        } else {
            format!("{root}/{directory}")
        };
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"root\"\r\n\r\n{root_field}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{basename}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        body.extend_from_slice(data);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        super::api::rest_post(
            &self.config,
            "/server/files/upload",
            &format!("multipart/form-data; boundary={boundary}"),
            &body,
        )
        .await?;
        if root == "gcodes" {
            let content = String::from_utf8_lossy(data);
            let mut session = self.session.lock().await;
            session.program_name = Some(filename.rsplit('/').next().unwrap_or(filename).to_owned());
            session.total_program_lines = content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .count() as u32;
            session.toolpath = crate::parse_toolpath(&content);
            session.script_line = 0;
        }
        Ok(())
    }

    /// Moves one path in Moonraker's selected file root.
    pub async fn move_file(&self, root: &str, source: &str, destination: &str) -> CncResult<()> {
        validate_root(root)?;
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.rpc(
            &mut session,
            "server.files.move",
            serde_json::json!({"root": root, "source": source, "dest": destination}),
        )
        .await
        .map(|_| ())
    }

    /// Deletes one file or directory in Moonraker's selected file root.
    pub async fn delete_file(&self, root: &str, path: &str) -> CncResult<()> {
        validate_root(root)?;
        let mut session = self.session.lock().await;
        self.ensure_connected(&mut session).await?;
        self.rpc(
            &mut session,
            "server.files.delete_file",
            serde_json::json!({"root": root, "path": path}),
        )
        .await
        .map(|_| ())
    }

    /// Downloads a file through Moonraker's REST endpoint without blocking the Tokio runtime.
    pub async fn download_file(&self, root: &str, path: &str) -> CncResult<Vec<u8>> {
        validate_root(root)?;
        let target = format!("/server/files/{root}/{}", path.trim_start_matches('/'));
        rest_get(&self.config, &target).await
    }
}

/// Rejects roots outside the two filesystems intentionally exposed by the plan.
fn validate_root(root: &str) -> CncResult<()> {
    if matches!(root, "gcodes" | "config") {
        Ok(())
    } else {
        Err(ControllerError::new(
            "bad_path",
            "Moonraker root must be `gcodes` or `config`",
        ))
    }
}

/// Parses a Moonraker file listing record.
fn parse_entry(value: &Value) -> Option<KlipperFileEntry> {
    Some(KlipperFileEntry {
        name: value.get("path")?.as_str()?.rsplit('/').next()?.to_owned(),
        is_dir: value.get("type").and_then(Value::as_str) == Some("directory"),
        size: value.get("size").and_then(Value::as_u64).unwrap_or(0),
    })
}

/// Downloads a REST resource and returns its body after validating the HTTP status.
async fn rest_get(config: &super::KlipperConfig, path: &str) -> CncResult<Vec<u8>> {
    let endpoint = config.endpoint.strip_prefix("http://").ok_or_else(|| {
        ControllerError::new(
            "unsupported",
            "Moonraker file download currently requires http://",
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
    .map_err(|_| ControllerError::new("timeout", "Moonraker download connection timed out"))?
    .map_err(|e| ControllerError::new("transport", e.to_string()))?;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let auth = config
        .api_key
        .as_ref()
        .map(|key| format!("X-Api-Key: {key}\r\n"))
        .unwrap_or_default();
    stream
        .write_all(
            format!(
                "GET {target} HTTP/1.1\r\nHost: {authority}\r\n{auth}Connection: close\r\n\r\n"
            )
            .as_bytes(),
        )
        .await
        .map_err(|e| ControllerError::new("transport", e.to_string()))?;
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .map_err(|e| ControllerError::new("transport", e.to_string()))?;
    let split = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| ControllerError::new("protocol", "malformed Moonraker HTTP response"))?;
    let headers = String::from_utf8_lossy(&response[..split]);
    if !headers.lines().next().unwrap_or_default().contains(" 2") {
        return Err(ControllerError::new(
            "moonraker_http",
            headers.lines().next().unwrap_or_default(),
        ));
    }
    Ok(response[split + 4..].to_vec())
}
