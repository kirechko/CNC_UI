//! Operating-system-specific process setup for the unified Linux/Windows application.

/// Applies platform-specific options before spawning the bundled emulator child.
#[cfg(target_os = "linux")]
pub(crate) fn prepare_emulator_command(command: &mut std::process::Command) {
    let _ = command;
}

/// Applies platform-specific options before spawning the bundled emulator child.
#[cfg(target_os = "windows")]
pub(crate) fn prepare_emulator_command(command: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;

    // Keep the hidden emulator worker from opening a second console window.
    command.creation_flags(0x08000000);
}

/// Rejects unsupported targets rather than producing a misleading partial build.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
compile_error!("cnc_ui currently supports Linux and Windows targets only");
