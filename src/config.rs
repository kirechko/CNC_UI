use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Backend {
    #[default]
    Emulator,
    LinuxCnc,
    Grbl,
    FluidNc,
    FluidNcRs232,
    FluidNcWebSocket,
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub backend: Backend,
    pub emulator: EmulatorSettings,
    pub linuxcnc: LinuxCncSettings,
    pub grbl: GrblSettings,
    pub fluidnc: FluidNcSettings,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            backend: Backend::Emulator,
            emulator: EmulatorSettings::default(),
            linuxcnc: LinuxCncSettings::default(),
            grbl: GrblSettings::default(),
            fluidnc: FluidNcSettings::default(),
        }
    }
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => {
                return Err(format!("ошибка чтения файла: {error}"));
            }
        };

        serde_yaml_ng::from_str(&content).map_err(|error| format!("некорректный YAML: {error}"))
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct EmulatorSettings {
    pub socket_path: String,
}

impl Default for EmulatorSettings {
    fn default() -> Self {
        Self {
            socket_path: "/tmp/linuxcnc-ui-emulator.sock".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct LinuxCncSettings {
    pub socket_path: String,
}

impl Default for LinuxCncSettings {
    fn default() -> Self {
        Self {
            socket_path: "/run/linuxcnc/linuxcnc.sock".into(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct GrblSettings {
    pub serial_port: String,
    pub baud_rate: u32,
}

impl Default for GrblSettings {
    fn default() -> Self {
        Self {
            serial_port: "/dev/serial/by-id/REPLACE_WITH_CONTROLLER_ID".into(),
            baud_rate: 115_200,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct FluidNcSettings {
    pub base_url: String,
    pub discovery_port: Vec<String>,
    pub serial_port: String,
    pub baud_rate: u32,
    pub websocket_rx_buffer_bytes: usize,
    pub websocket_max_line_len: usize,
    pub websocket_ack_timeout_ms: u64,
    pub websocket_status_timeout_ms: u64,
    pub websocket_connect_timeout_ms: u64,
}

impl Default for FluidNcSettings {
    fn default() -> Self {
        Self {
            base_url: "http://192.168.0.10".into(),
            discovery_port: Vec::new(),
            serial_port: "/dev/serial/by-id/REPLACE_WITH_CONTROLLER_ID".into(),
            baud_rate: 115_200,
            websocket_rx_buffer_bytes: 128,
            websocket_max_line_len: 80,
            websocket_ack_timeout_ms: 2_000,
            websocket_status_timeout_ms: 1_000,
            websocket_connect_timeout_ms: 5_000,
        }
    }
}
