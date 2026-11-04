//! Focused unit tests for serial discovery identity and candidate rules.

use super::identity::is_supported_grbl_version;
use super::scan::is_serial_candidate;
use super::*;
use crate::discovery::DiscoveryTransport;

/// Ensures a GRBL-shaped boot line is not treated as FluidNC identity.
#[test]
fn grbl_boot_banner_remains_tentative_until_fluidnc_identity_is_checked() {
    let banner = "Grbl 1.1h ['$' for help]";
    let fluidnc_identity = "[VER:FluidNC@v1.3.11:Debug]";

    assert_eq!(parse_banner_version(banner).as_deref(), Some("1.1h"));
    assert_eq!(fluidnc_banner_version(banner), None);
    assert_eq!(
        fluidnc_banner_version(fluidnc_identity).as_deref(),
        Some("1.3.11")
    );
}

#[test]
fn banner_version_parses_the_firmware_token_only() {
    assert_eq!(
        parse_banner_version("Grbl 1.1h ['$' for help]").as_deref(),
        Some("1.1h")
    );
    assert_eq!(parse_banner_version("Grbl 0.9j").as_deref(), Some("0.9j"));
    assert_eq!(parse_banner_version("ok"), None);
    assert_eq!(
        parse_banner_version("<Idle|MPos:0.000,0.000,0.000|FS:0,0>"),
        None
    );
    assert_eq!(parse_banner_version("Grblby"), None);
}

#[test]
fn fluidnc_banner_version_reads_the_identity_token() {
    assert_eq!(
        fluidnc_banner_version("[VER:FluidNC@v1.3.11:Debug]").as_deref(),
        Some("1.3.11")
    );
    assert_eq!(
        fluidnc_banner_version("[VER:FluidNC 1.2.0]").as_deref(),
        Some("1.2.0")
    );
    // GRBL banners and unrelated lines must not match.
    assert_eq!(fluidnc_banner_version("Grbl 1.1h ['$' for help]"), None);
    assert_eq!(fluidnc_banner_version("[MSG:Workspace:0]"), None);
}

#[test]
fn extract_fluidnc_ipv4_finds_the_address_after_the_ip_marker() {
    let stream = "[Boot:FluidNC v1.3.11]\n[MSG:WiFi Connected, IP:192.168.1.42]\n";
    assert_eq!(
        extract_fluidnc_ipv4(stream),
        Some("192.168.1.42".parse().unwrap())
    );
    // No IP in the banner text: the serial record path must be used.
    assert_eq!(
        extract_fluidnc_ipv4("[VER:FluidNC@v1.3.11:Debug]\nok\n"),
        None
    );
}

#[test]
fn fluidnc_serial_device_id_follows_the_basename_convention() {
    assert!(fluidnc_serial_device_id("/dev/ttyUSB1").starts_with("fluidnc:serial:ttyUSB1"));
}

#[test]
fn only_grbl_1x_firmware_is_supported() {
    assert!(is_supported_grbl_version(Some("1.1h")));
    assert!(is_supported_grbl_version(Some("1.r2")));
    assert!(!is_supported_grbl_version(Some("0.9j")));
    // Status-only detection has no version and stays acceptable.
    assert!(is_supported_grbl_version(None));
}

#[test]
fn candidate_filter_accepts_common_grbl_adapters_and_skips_others() {
    for name in ["/dev/ttyUSB0", "/dev/ttyACM0", "/dev/ttyS0", "COM5"] {
        assert!(is_serial_candidate(name), "{name}");
    }
    for name in [
        "/dev/ttyprintk",
        "/dev/rfcomm0",
        "/dev/ttyUSB0-modem/Bluetooth",
        "/dev/pts/3",
    ] {
        assert!(!is_serial_candidate(name), "{name}");
    }
}

#[test]
fn device_id_and_endpoint_keep_the_serial_conventions() {
    let device = grbl_serial_device("/dev/ttyUSB0", Some("1.1h".into()));
    // The id key is the canonical basename so it matches
    // GrblAdapter::connected_device_id; the endpoint is the full path.
    assert_eq!(device.id, grbl_serial_device_id("/dev/ttyUSB0"));
    assert!(
        device.id.starts_with("grbl:serial:ttyUSB0"),
        "{}",
        device.id
    );
    assert!(device.endpoint.contains("ttyUSB0"));
    assert_eq!(device.protocol, "GRBL");
    assert_eq!(device.transport, DiscoveryTransport::Serial);
    assert_eq!(device.version.as_deref(), Some("1.1h"));
}
