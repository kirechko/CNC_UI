//! Glue layer between the application and the `rosetta` crate (T26).
//!
//! Design rule from `plans/Rosetta-integration-plan.md`: `rosetta/` itself is
//! never modified; everything the app needs plugs into the pre-designed
//! synchronous seams (`HostRuntime`, `MacroRegistry`, `Pipeline`,
//! `CapabilityMatrix`) from this module. The submodules split by seam:
//!
//! * [`backend`]     — maps the connected machine's id onto
//!   `rosetta::ir::capability::BackendType`.
//! * [`runtime`]     — [`runtime::ControllerRuntime`], the `HostRuntime`
//!   implementation that drives the machine through `CNCController`'s
//!   blocking `Execute` command path, plus the machine-state mapping and the
//!   status-field vocabulary (`pos.x`, `state`, `idle`, …).
//! * [`registry`]    — converts the SQLite `user_macros` rows (flat G-code
//!   bodies) into `rosetta` `MacroProgram`s and assembles a `MacroRegistry`.
//!   No schema migration: a `rosetta_yaml` column is added idempotently and
//!   stays empty for legacy flat bodies.
//! * [`transform`]   — the `Pipeline` bridge: capability validation of a
//!   program text, plan suggestions ("preview") and applying the plan
//!   ("apply"), including the `MacroExpander` implementation `CycleSubstitute`
//!   needs.
//!
//! The module is compiled into every binary that declares it: the axum web
//! backend (`src/bin/cnc-web.rs`), the combined desktop/web binary
//! (`src/main.rs`), and the Tauri app through `crate::web_server` re-exports.

pub mod backend;
pub mod registry;
pub mod run;
pub mod runtime;
pub mod transform;
