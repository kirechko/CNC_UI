//! G-code program upload: validation, temp saving and controller loading.
//!
//! `load_program` is called by both the axum handler (`POST /api/program`)
//! and the Tauri command, so the validation rules live here once.

use std::fs;
use std::path::Path;

use super::models::LoadedProgram;
use super::state::position::temp_program_path;
use super::state::AppState;

/// Maximum accepted program size, in bytes.
const MAX_PROGRAM_BYTES: usize = 5 * 1024 * 1024;

/// Body of `POST /api/program` (camelCase `fileName` matches the browser).
#[derive(serde::Deserialize)]
pub(crate) struct ProgramRequest {
    #[serde(rename = "fileName")]
    pub(crate) file_name: String,
    pub(crate) content: String,
}

/// Error raised while validating or saving a G-code program.
///
/// `internal` marks server-side failures (I/O, poisoned locks) that map to HTTP 500;
/// all other failures are caused by the client input and map to HTTP 400.
#[derive(Debug)]
pub(crate) struct ProgramError {
    pub(crate) internal: bool,
    pub(crate) message: String,
}

impl ProgramError {
    /// Creates an error caused by invalid client input or machine state.
    pub(crate) fn client(message: String) -> Self {
        Self {
            internal: false,
            message,
        }
    }

    /// Creates an error caused by a server-side failure.
    pub(crate) fn internal(message: String) -> Self {
        Self {
            internal: true,
            message,
        }
    }
}

/// Validates the file name, extension and size of an uploaded program.
/// Error messages are translated into `language`.
fn validate_program(name: &str, content: &str, language: &str) -> Result<(), ProgramError> {
    if name.is_empty() || name.len() > 255 || name.contains(['/', '\\']) {
        return Err(ProgramError::client(crate::i18n::translate(
            language,
            "api.error.fileName",
            &[],
        )));
    }
    let extension = Path::new(name)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !["nc", "ngc", "gcode"].contains(&extension.as_str()) {
        return Err(ProgramError::client(crate::i18n::translate(
            language,
            "api.error.fileExtension",
            &[],
        )));
    }
    if content.len() > MAX_PROGRAM_BYTES {
        return Err(ProgramError::client(crate::i18n::translate(
            language,
            "api.error.programSize",
            &[],
        )));
    }
    Ok(())
}

/// Validates, saves and loads a G-code program into the controller.
/// `language` is used to translate client-facing validation messages.
pub(crate) fn load_program(
    state: &AppState,
    name: &str,
    content: &str,
    language: &str,
) -> Result<LoadedProgram, ProgramError> {
    validate_program(name, content, language)?;

    let path = temp_program_path();
    fs::write(&path, content).map_err(|error| {
        ProgramError::internal(format!(
            "Failed to save G-code to {}: {error}",
            path.display()
        ))
    })?;

    let controller = state
        .controller
        .lock()
        .map_err(|_| ProgramError::internal("controller lock poisoned".to_string()))?;
    controller
        .load_program(&path)
        .map_err(|error| ProgramError::client(format!("{}: {}", error.code, error.message)))?;

    let program = LoadedProgram {
        name: name.to_string(),
        content: content.to_string(),
    };
    *state
        .program
        .lock()
        .map_err(|_| ProgramError::internal("program lock poisoned".to_string()))? =
        Some(program.clone());

    Ok(program)
}
