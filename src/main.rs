//! Unified command-line entry point for FerroHUB web, desktop, and optional emulator modes.
use cnc_core as cnc_api;
mod cnc_controller;
mod emulator;
mod ferrohub;
mod i18n;
mod message_log;
mod rosetta_host;
#[cfg(feature = "desktop")]
mod tauri_app;
mod web_server;

/// Selects the frontend served by the unified application executable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RunMode {
    Web,
    Desktop,
}

/// Captures the validated command-line options for one application run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RunOptions {
    mode: RunMode,
    with_emulator: bool,
}

/// Represents either a normal run request or a request to print command-line help.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CliAction {
    Run(RunOptions),
    Help,
}

/// Parses supported flags, applies the desktop default, and rejects ambiguous invocations.
fn parse_args<I, S>(arguments: I) -> Result<CliAction, String>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    let mut mode = None;
    let mut with_emulator = false;
    for argument in arguments.into_iter().map(Into::into) {
        match argument.as_str() {
            "--web" if mode.is_none() => mode = Some(RunMode::Web),
            "--desktop" if mode.is_none() => mode = Some(RunMode::Desktop),
            "--emu" if !with_emulator => with_emulator = true,
            "--help" | "-h" => return Ok(CliAction::Help),
            "--web" | "--desktop" | "--emu" => {
                return Err(format!("Conflicting or repeated argument: {argument}"));
            }
            unknown => return Err(format!("Unknown argument: {unknown}")),
        }
    }
    Ok(CliAction::Run(RunOptions {
        mode: mode.unwrap_or(RunMode::Desktop),
        with_emulator,
    }))
}

/// Parses CLI arguments and reports startup failures using a nonzero process exit code.
fn main() {
    match run() {
        Ok(()) => {}
        Err(error) => {
            eprintln!("Application failed: {error}");
            std::process::exit(1);
        }
    }
}

/// Starts the optional in-process emulator before the selected frontend and stops it afterward.
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let options = match parse_args(std::env::args().skip(1)).map_err(std::io::Error::other)? {
        CliAction::Help => {
            println!("{} — CNC machine control", ferrohub::PRODUCT_NAME);
            println!("Usage: cnc_ui [--web | --desktop] [--emu]");
            return Ok(());
        }
        CliAction::Run(options) => options,
    };

    let emulator = if options.with_emulator {
        let address =
            std::env::var("CNC_EMULATOR_ADDRESS").unwrap_or_else(|_| "127.0.0.1:8765".to_string());
        let config_path = std::env::var_os("CNC_EMULATOR_CONFIG")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("config/emulator.yaml"));
        Some(emulator::EmulatorHandle::start(address, config_path).map_err(std::io::Error::other)?)
    } else {
        None
    };

    let frontend_result = match options.mode {
        RunMode::Web => run_web(),
        RunMode::Desktop => run_desktop(),
    };
    if let Some(emulator) = emulator {
        emulator.request_shutdown();
        frontend_result?;
        emulator.join().map_err(std::io::Error::other)?;
    } else {
        frontend_result?;
    }
    Ok(())
}

/// Runs the web frontend and converts Ctrl+C into Axum graceful shutdown.
fn run_web() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        web_server::server::run_with_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
    })?;
    Ok(())
}

/// Starts Tauri when desktop support was enabled for this platform build.
#[cfg(feature = "desktop")]
fn run_desktop() -> Result<(), Box<dyn std::error::Error>> {
    tauri_app::run()?;
    Ok(())
}

/// Reports a clear error for browser-only builds that lack the Tauri desktop feature.
#[cfg(not(feature = "desktop"))]
fn run_desktop() -> Result<(), Box<dyn std::error::Error>> {
    Err("This binary was built without desktop support; rebuild with --features desktop".into())
}

#[cfg(test)]
mod tests {
    use super::{parse_args, CliAction, RunMode, RunOptions};

    /// Uses desktop mode when no frontend flag is supplied and leaves the emulator disabled.
    #[test]
    fn defaults_to_desktop_without_emulator() {
        assert_eq!(
            parse_args(std::iter::empty::<&str>()).unwrap(),
            CliAction::Run(RunOptions {
                mode: RunMode::Desktop,
                with_emulator: false,
            })
        );
    }

    /// Accepts frontend and emulator flags regardless of their order.
    #[test]
    fn accepts_modes_and_optional_emulator_in_any_order() {
        assert_eq!(
            parse_args(["--emu", "--web"]).unwrap(),
            CliAction::Run(RunOptions {
                mode: RunMode::Web,
                with_emulator: true,
            })
        );
        assert_eq!(
            parse_args(["--desktop", "--emu"]).unwrap(),
            CliAction::Run(RunOptions {
                mode: RunMode::Desktop,
                with_emulator: true,
            })
        );
    }

    /// Rejects duplicate options and invocations that select both frontend modes.
    #[test]
    fn rejects_repeated_and_conflicting_flags() {
        for arguments in [
            vec!["--web", "--web"],
            vec!["--desktop", "--desktop"],
            vec!["--emu", "--emu"],
            vec!["--web", "--desktop"],
            vec!["--desktop", "--web"],
        ] {
            assert!(parse_args(arguments).is_err());
        }
    }

    /// Rejects obsolete private-worker flags and all unrecognized options.
    #[test]
    fn rejects_unknown_arguments() {
        assert!(parse_args(["--emu-child"]).is_err());
        assert!(parse_args(["--unknown"]).is_err());
    }

    /// Returns help as an action without starting either frontend or the emulator.
    #[test]
    fn help_short_circuits_other_flags() {
        assert_eq!(parse_args(["--emu", "--help"]).unwrap(), CliAction::Help);
        assert_eq!(parse_args(["-h"]).unwrap(), CliAction::Help);
    }
}
