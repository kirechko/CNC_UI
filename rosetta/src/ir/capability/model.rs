use serde::{Deserialize, Serialize};

use crate::ir::macro_ir::{MacroId, TransformId};
use crate::ir::toolpath::Code;

/// Backend families known to the system (ТЗ №1 §3.3). The `Default` is
/// [`BackendType::Emulator`] — the safest choice for a freshly constructed
/// emitter (no hardware assumptions, everything supported).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum BackendType {
    LinuxCnc,
    Klipper,
    Grbl,
    FluidNc,
    #[default]
    Emulator,
}

impl BackendType {
    /// Canonical short label used in error messages and reports.
    pub fn label(self) -> &'static str {
        match self {
            BackendType::LinuxCnc => "LinuxCNC",
            BackendType::Klipper => "Klipper",
            BackendType::Grbl => "GRBL",
            BackendType::FluidNc => "FluidNC",
            BackendType::Emulator => "Emulator",
        }
    }
}

impl std::fmt::Display for BackendType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Language constructs of Macro IR checked against a backend (spec §5
/// matrix). `Ord` exists so safety scans can collect features in a `BTreeSet`
/// deterministically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Feature {
    /// Plain sequence of interpolated G-code lines.
    Sequence,
    /// `Stmt::Delay` — rendered as a dwell, always portable.
    Delay,
    /// `Stmt::Wait` — needs live feedback, impossible in offline expansion.
    WaitStatus,
    /// `Stmt::SetVar` / `Expr::Var` — host-side variables.
    Variables,
    /// `Stmt::If` branching.
    IfElse,
    /// `Stmt::While` / `Stmt::Repeat` loops.
    WhileRepeat,
    /// `Stmt::ForEach` + `Expr::Array`/`Range` data.
    ForeachArrays,
    /// `Stmt::Call` recursion (a sub calling itself transitively).
    Recursion,
    /// Named subprograms (`Subroutine`) with parameter passing.
    Subprograms,
    /// `Stmt::InvokeTransform` — cross-call into Rosetta Transform.
    InvokeTransform,
}

/// How well a backend supports something (ТЗ №1 §3.3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Support {
    /// Fully supported.
    Full,
    /// Supported with caveats described in the attached note.
    Partial(String),
    /// Cannot be expressed at all — must be resolved or rejected.
    None,
}

impl Support {
    /// Convenience predicate for callers that only gate on "can run it".
    pub fn supported(&self) -> bool {
        !matches!(self, Support::None)
    }
}

/// How serious an incompatibility is for the pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational — pass-through is fine, the note explains a nuance.
    Info,
    /// A resolution exists; applying it needs operator confirmation.
    Warning,
    /// The backend cannot express the code and no resolution is registered —
    /// the plan is not executable until the user fixes it.
    Blocker,
}

/// A concrete way to eliminate an incompatibility (ТЗ №1 §3.3, §7 of ТЗ №3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    /// Replace the offending code with an invocation of the named macro
    /// (`SubstituteMacro`, executed by the Macro Engine — TRF-M-01/02).
    SubstituteMacro(MacroId),
    /// Rewrite the geometry/codes with a transform of the named kind
    /// (`ApplyTransform`, executed by Rosetta Transform — TRF-C-05).
    ApplyTransform(TransformId),
    /// Silently drop the code from the emitted program.
    Remove,
    /// No automatic fix: warn the operator and stop.
    Manual,
}

/// Confirmation strictness for resolutions and transform plans (ТЗ №3 §10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmPolicy {
    /// Confirm every step that is not provably safe.
    #[default]
    EachUnsafe,
    /// Show one combined preview ("was/became") for the whole plan.
    OnceForPlan,
    /// Apply without prompts — only acceptable for non-geometric steps.
    Auto,
}

/// A registered rule mapping `(backend, code)` to a `Resolution`
/// (`CORE-T-05`, `TRF-C-05`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolutionRule {
    pub backend: BackendType,
    pub code: Code,
    pub resolution: Resolution,
    /// Strictness of the operator gate when this rule is used.
    pub confirm: ConfirmPolicy,
    /// Optional human note ("emulate peck drilling with macro", …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One finding of the incompatibility scanner (`CORE-T-04`, `TRF-C-04`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Incompatibility {
    pub backend: BackendType,
    pub code: Code,
    /// 1-based source line where the code appears.
    pub line: usize,
    pub severity: Severity,
    /// The registered fix, if any (`resolution_for`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<Resolution>,
    /// Why this was flagged (support note or rule note).
    pub detail: String,
}
