//! Rosetta — the umbrella system defined by the three specifications in
//! `plans/Rosetta.md`, `plans/Macros v3.md` and `plans/Rosetta_transform.md`.
//!
//! ```text
//!                          Rosetta (umbrella brand)
//!   ┌─────────────────────────────────────────────────────────────────┐
//!   │  CORE — Rosetta IR (this crate, `rosetta::ir`)                  │
//!   │  • Macro IR (MacroProgram / Stmt / Expr)   • Toolpath IR        │
//!   │  • Capability matrix + Resolution registry                      │
//!   │  • Frontends (YAML, G-code) and backends (emitters)             │
//!   └───────────────▲───────────────────────────────▲─────────────────┘
//!          uses                             uses
//!   ┌───────────────────────────┐   ┌─────────────────────────────────┐
//!   │ MACRO ENGINE  (ТЗ №2)     │   │ ROSETTA TRANSFORM   (ТЗ №3)     │
//!   │ `rosetta::macro_engine`   │◄─►│ `rosetta::transform`            │
//!   │ generative/reactive       │   │ affine / geometric / structural │
//!   └───────────────────────────┘   └─────────────────────────────────┘
//! ```
//!
//! # Contract traceability
//!
//! Every public type or function documents the specification contract IDs it
//! implements (`CORE-M-01`, `MAC-C-03`, `TRF-C-01`, …) so the cross-reference
//! matrix at the end of `Rosetta.md` can be verified against code.
//!
//! # Deliberate deviations from the ТЗ wording
//!
//! * The ТЗ sketches async traits (`async fn invoke(...)`). This core crate is
//!   **synchronous**: host I/O is injected through [`macro_engine::runtime::HostRuntime`]
//!   and transform/macro cross-calls use plain `FnMut` hooks. An async facade
//!   can be added by the application layer without changing any IR types, and
//!   the crate stays free of a runtime dependency (portability goal).
//! * `Resolution::ApplyTransform` carries a transform *kind name* (`String`)
//!   rather than the full `TransformKind`, to keep `rosetta::ir` free of the
//!   transform module (no cyclic coupling). The registry documents valid names.
//!
//! # Phases implemented here
//!
//! * 🟢 Core Phase 1: Macro IR + YAML frontend + capability matrix + flat-code
//!   emitter; Toolpath IR + G-code parse/emit contract (`ToolpathIo`).
//! * 🟢 Macro Engine Phase 1: registry, triggers model, host interpreter
//!   (reactive), generative expansion, dry-run, safety pre-start checks.
//! * 🟢 Transform Phase A: affine transforms (Rotate/Translate/Scale/Mirror/
//!   UnitsConvert) over the compatibility pipeline with confirmation policy,
//!   artifacts and tracing; structural `FeedSpeedOverride`, `CoordSystemRemap`,
//!   `StripUnsupported`, `CycleSubstitute` (via macro invoker hook).
//! * Deferred stubs return `RosettaError::Unsupported` with the phase label:
//!   O-words frontend, Klipper/Jinja2 emitter, CutterCompBake, regions by marks.

pub mod error;
pub mod ir;
pub mod io;
pub mod macro_engine;
pub mod transform;
pub mod util;

pub use error::{RosettaError, RosettaResult};

/// Contract version of the public interfaces (spec `CORE-V-01`).
///
/// Breaking changes to `MacroProgram`, `Stmt`, `Expr`, `Toolpath`, `Segment`
/// or any trait below MUST bump this number, and the macro engine and
/// transform modules of this crate must be updated in the same commit.
pub const CONTRACT_VERSION: u32 = 1;
