//! Maps the application's connected machine onto a Rosetta `BackendType`.
//!
//! Discovery device ids follow the canonical `<protocol>:<transport>:<endpoint>`
//! scheme produced by `cnc_core::discovery` (`emulator:tcp_socket:…`,
//! `fluidnc:WebSocket:…`, `grbl:serial:…`), so the prefix is a stable
//! discriminator for the capability matrix and the transform pipeline.

use rosetta::ir::capability::BackendType;

/// Backend for a discovery device id (as returned by
/// `CNCController::connected_device_id`). Unknown or missing ids fall back to
/// `Emulator` — the most permissive matrix row, matching the app's boot
/// default (the UI starts against the local emulator adapter).
pub fn backend_from_device_id(device_id: Option<&str>) -> BackendType {
    match device_id {
        Some(id) if id.starts_with("grbl:") => BackendType::Grbl,
        Some(id) if id.starts_with("fluidnc:") => BackendType::FluidNc,
        // LinuxCNC control is not wired yet, but the namespace is reserved so
        // the mapping stays forward-compatible with the NML adapter.
        Some(id) if id.starts_with("linuxcnc:") => BackendType::LinuxCnc,
        _ => BackendType::Emulator,
    }
}

/// Parses the lowercase backend token used in `rosetta_settings` preset keys
/// (`substitute:grbl:G83`), matching the seed vocabulary in
/// `web_server::state::seed`. Unknown tokens return `None` so a malformed
/// preset row is skipped instead of silently retargeting another backend.
pub fn backend_from_key(token: &str) -> Option<BackendType> {
    match token {
        "linuxcnc" => Some(BackendType::LinuxCnc),
        "klipper" => Some(BackendType::Klipper),
        "grbl" => Some(BackendType::Grbl),
        "fluidnc" => Some(BackendType::FluidNc),
        "emulator" => Some(BackendType::Emulator),
        _ => None,
    }
}

/// Unit tests for the id-prefix mapping used by every Rosetta endpoint.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_prefixes_map_to_their_backends() {
        assert_eq!(
            backend_from_device_id(Some("grbl:serial:ttyUSB0")),
            BackendType::Grbl
        );
        assert_eq!(
            backend_from_device_id(Some("fluidnc:WebSocket:192.168.1.5:80")),
            BackendType::FluidNc
        );
        assert_eq!(
            backend_from_device_id(Some("emulator:tcp_socket:127.0.0.1:8765")),
            BackendType::Emulator
        );
    }

    #[test]
    fn missing_or_unknown_ids_default_to_emulator() {
        assert_eq!(backend_from_device_id(None), BackendType::Emulator);
        assert_eq!(
            backend_from_device_id(Some("wat:thing")),
            BackendType::Emulator
        );
    }
}
