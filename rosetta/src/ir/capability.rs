//! Capability Matrix + single Resolution Registry (`CORE-M-03`, `CORE-T-03`,
//! `CORE-T-04`, `CORE-V-02`).
//!
//! The matrix answers two questions:
//!
//! 1. *Can a backend express a language construct?* — [`Feature`] axis, used
//!    by the Macro Engine before running or emitting a program (`CORE-M-03`,
//!    enforced by `MAC-C-07`: no execution without a passed check).
//! 2. *Can a backend run a specific G/M code?* — [`Code`] axis, used by the
//!    Transform detector (`TRF-C-03`).
//!
//! When the answer is "no", the *Resolution Registry* says how to fix it:
//! substitute a macro, apply a transform, drop the code, or tell the human
//! (`CORE-T-04`). Per `CORE-V-02` this is the ONLY resolution table in the
//! system — neither `macro_engine` nor `transform` keeps its own copy.
//!
//! # Deviations
//!
//! * The spec table has separate "GRBL generative" and "GRBL reactive"
//!   columns although `BackendType::Grbl` is one variant. Rather than
//!   duplicating backends, [`CapabilityRegistry::supports_feature`] takes the
//!   [`MacroClass`] as an extra parameter so the GRBL row is resolved by the
//!   calling engine (a generative GRBL expansion cannot `wait` for status, a
//!   reactive host interpreter can).
//! * `Resolution::ApplyTransform` carries the transform *kind name* as a
//!   `String` instead of the full `TransformKind`, to keep this module free
//!   of the `transform` submodule (no cyclic coupling). Valid names are the
//!   serde names of `rosetta::transform::model::TransformKind` variants.

mod model;
mod registry;

pub use model::{
    BackendType, ConfirmPolicy, Feature, Incompatibility, Resolution, ResolutionRule, Severity,
    Support,
};
pub use registry::{CapabilityMatrix, CapabilityRegistry};
