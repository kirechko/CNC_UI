//! Rosetta Transform (ТЗ №3) — the offline program-conversion engine.
//!
//! Rewrites an existing G-code program so a target controller can run it,
//! or applies geometric/technological operations, always with operator
//! oversight. Where the Macro Engine substitutes *frames*, Transform analyzes
//! and recomputes the *whole trajectory* — hence its own module over the
//! core's Toolpath IR (`TRF-C-01`).
//!
//! # Layout
//!
//! * [`model`] — data schema: [`model::TransformKind`] (affine / geometric /
//!   structural families), [`model::Region`], [`model::TransformStep`],
//!   [`model::TransformPlan`], [`model::Artifact`] + the [`model::Transform`]
//!   and [`model::MacroExpander`] trait seams.
//! * [`affine`] — `Rotate`/`Translate`/`Scale`/`Mirror`/`UnitsConvert` over a
//!   single 2D matrix + scalar Z rule (Phase A).
//! * [`structural`] — `FeedSpeedOverride`/`CoordSystemRemap`/
//!   `CycleSubstitute`/`StripUnsupported` (Phases B/C) and the explicit
//!   Phase-D stubs for the geometric family.
//! * [`pipeline`] — the §5 stage machine
//!   Load→Parse→Detect→Plan→Confirm→Apply→Validate→Emit→Artifact.
//! * [`registry`] — the [`registry::TransformRegistry`] catalog satisfying
//!   `TRF-M-04` (link validation for `InvokeTransform`) and `TRF-M-03`
//!   (programmatic invocation by id + positional args).
//!
//! # Cross-subsystem contracts
//!
//! * `TRF-C-03/04`: incompatibility detection is fully delegated to the
//!   core's capability scan — this module keeps no private table.
//! * `TRF-C-05`: the core's built-in `Resolution::ApplyTransform` rules name
//!   kinds by their catalog id (the serde variant name, e.g. `"Rotate"`);
//!   `ir::capability` therefore does not import this module (no cycle).
//! * `TRF-M-01/02`: `CycleSubstitute` needs macro expansion; the pipeline
//!   receives it through the narrow [`model::MacroExpander`] trait so the
//!   application wires `macro_engine::generative::expand_program` without
//!   transform importing the engine.
//! * `TRF-M-05/06`: substitution emulators must be stateless, and the
//!   artifact trace always references the *original* source line.

pub mod affine;
pub mod model;
pub mod pipeline;
pub mod registry;
pub mod structural;

pub use affine::AffineTransform;
pub use model::{
    Artifact, CompSide, CoordSystem, CycleCode, LeadKind, LeadSpec, Limits, MacroExpander,
    MirrorAxes, ProgramRef, Region, TraceEntry, Transform, TransformFamily, TransformKind,
    TransformPlan, TransformReport,
};
pub use pipeline::{Pipeline, PlanConfirmation};
pub use registry::{TransformRef, TransformRegistry, TransformSpec};
pub use structural::{GeometricStub, StructuralTransform};
