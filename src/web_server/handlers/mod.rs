//! Axum HTTP handlers.
//!
//! Handlers stay intentionally thin: they extract/validate the request, then
//! delegate to the operation methods on `AppState` (`state/`) or to the
//! machine/program modules. Error responses are `(StatusCode, String)` pairs,
//! with user-facing messages already translated into the request language
//! (see `models::request_language`).

mod console;
mod control;
mod files;
mod machine_profiles;
mod macros;
mod preflight;
mod rosetta;
mod tool_library;
mod tool_types;

pub(crate) use console::*;
pub(crate) use control::*;
pub(crate) use files::*;
pub(crate) use machine_profiles::*;
pub(crate) use macros::*;
pub(crate) use preflight::*;
pub(crate) use rosetta::*;
pub(crate) use tool_library::*;
pub(crate) use tool_types::*;
