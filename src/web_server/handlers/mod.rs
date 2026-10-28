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

pub(crate) use console::*;
pub(crate) use control::*;
pub(crate) use files::*;
