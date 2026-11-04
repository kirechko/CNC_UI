//! CNC adapter implementations grouped by firmware and transport family.

pub mod debug;
pub mod emulator;
pub mod fluidnc_websocket;
#[path = "grbl_async/mod.rs"]
pub mod grbl;
pub mod grbl_mega;
pub mod klipper;
pub mod rep_rap_firmware;
pub mod serial_compat;
pub mod smoothie;
pub mod uccnc_bridge;

/// GRBL-family serial transport also used for FluidNC UART connections.
pub type FluidNcRs232Adapter = grbl::GrblAdapter;
/// Duet3D product-family name for the RepRapFirmware HTTP adapter.
pub type Duet3dAdapter = rep_rap_firmware::RepRapFirmwareAdapter;
/// TinyG and TinyG2 share JSON-line serial protocol handling.
pub type TinyGAdapter = serial_compat::SerialCompatAdapter;
/// Marlin protocol adapter uses the Marlin dialect of the compatibility serial driver.
pub type MarlinAdapter = serial_compat::SerialCompatAdapter;
