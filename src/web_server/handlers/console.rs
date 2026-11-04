//! Compatibility facade for the focused console HTTP handler modules.
//!
//! Re-exporting each endpoint keeps existing `handlers::console::*` paths stable.

mod console_log;
mod program;
mod run;
mod status;

pub(crate) use console_log::*;
pub(crate) use program::*;
pub(crate) use run::*;
pub(crate) use status::*;
