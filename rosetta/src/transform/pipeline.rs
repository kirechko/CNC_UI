//! The transform pipeline (ТЗ №3 §5):
//!
//! ```text
//! Load → Parse(Toolpath IR) → Detect(Capability Matrix) → Plan
//!      → Confirm(предпросмотр «было/стало») → Apply → Validate
//!      → Emit → Artifact
//! ```
//!
//! Synchronous deviation from the async ТЗ sketch (crate-level note): the
//! host UI drives the stages one method at a time; [`Pipeline::run`] is the
//! one-call form used by batch processing and tests.
//!
//! Contracts honored here:
//! * `TRF-C-03/04` — detection delegates to the core scanner, never a local table.
//! * `TRF-C-06` — mutation only through [`crate::transform::model::Transform`]
//!   executors that report affected source lines; the artifact keeps them.
//! * `TRF-M-06` — `CycleSubstitute` output lines replace the original text at
//!   emission while the frame (line number, codes) stays for tracing.
//! * §11.2 — the confirm stage is a *hard gate*: an unsafe step without the
//!   operator's approval aborts the run with `Validation` before any apply.
//! * §11.3 — validate stage: soft axis limits, path integrity, re-scan for
//!   remaining blockers. Dry-run on the emulator is the host's job (the
//!   macro engine's `DryRunRuntime` is the tool for it).

use crate::error::{RosettaError, RosettaResult};
use crate::io::gcode::GcodeIo;
use crate::io::ToolpathIo;
use crate::ir::capability::{
    BackendType, CapabilityMatrix, CapabilityRegistry, ConfirmPolicy, Incompatibility, Resolution,
    Severity,
};
use crate::ir::toolpath::Toolpath;
use crate::transform::model::{
    Artifact, CycleCode, Limits, MacroExpander, ProgramRef, TraceEntry, TransformKind,
    TransformPlan, TransformReport, TransformStep,
};
use crate::transform::registry::TransformRegistry;
use crate::transform::structural;

mod confirmation;
mod execution;
mod planning;

/// How the operator answered the confirm gate of one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanConfirmation<'a> {
    /// No interaction — legal only when the plan contains no
    /// `requires_confirm` step (or the policy is `Auto`).
    Auto,
    /// One blanket approval for the whole plan (`OnceForPlan`; also accepted
    /// for `EachUnsafe` as the UI's "approve all" gesture).
    PlanLevel { approved: bool },
    /// Explicit per-step indices approved by the operator.
    EachUnsafe { approvals: &'a [usize] },
}

/// Immutable input bundle of [`Pipeline::run`] (everything `Load`+`Parse`
/// produced, kept for the artifact's provenance). Not `Debug`: it holds a
/// trait-object reference (`expander`); use the field names in logs instead.
#[derive(Clone)]
pub struct Pipeline<'a> {
    /// Text frontend of the Toolpath IR (`CORE-T-02`).
    pub io: GcodeIo,
    /// Core capability matrix used for detection and validation (`TRF-C-03`).
    pub matrix: &'a CapabilityMatrix,
    /// Catalog that validates step ids and decodes `InvokeTransform` args.
    pub registry: &'a TransformRegistry,
    /// Optional macro bridge for `CycleSubstitute` (`TRF-M-01/02`).
    pub expander: Option<&'a dyn MacroExpander>,
    /// Optional soft limits enforced by the validate stage.
    pub limits: Option<Limits>,
}

impl<'a> Pipeline<'a> {
    /// Assemble a pipeline with all its collaborators.
    pub fn new(
        matrix: &'a CapabilityMatrix,
        registry: &'a TransformRegistry,
        expander: Option<&'a dyn MacroExpander>,
        limits: Option<Limits>,
    ) -> Self {
        Pipeline {
            io: GcodeIo::new(),
            matrix,
            registry,
            expander,
            limits,
        }
    }

    /// Stage *Load→Parse* (`TRF-C-02`).
    pub fn parse(&self, source: &str) -> RosettaResult<Toolpath> {
        self.io.parse(source)
    }

    /// Stage *Detect* (`TRF-C-04`): findings straight from the core scanner.
    pub fn detect(&self, toolpath: &Toolpath, backend: BackendType) -> Vec<Incompatibility> {
        self.matrix.scan_incompatibilities(toolpath, backend)
    }
}

#[cfg(test)]
mod tests;
