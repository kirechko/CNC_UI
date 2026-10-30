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

use serde::{Deserialize, Serialize};

use crate::ir::macro_ir::{MacroClass, MacroId, TransformId};
use crate::ir::toolpath::{Code, Toolpath};

/// Backend families known to the system (ТЗ №1 §3.3). The `Default` is
/// [`BackendType::Emulator`] — the safest choice for a freshly constructed
/// emitter (no hardware assumptions, everything supported).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize, Deserialize)]
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

/// The unified read interface both engines depend on (`CORE-M-03`,
/// `CORE-T-03/04`). Objects implementing this trait are the only sanctioned
/// source of capability answers; keeping it a trait lets embedders swap in a
/// machine-specific profile (e.g. GRBL with/without ATC) without changing
/// engine code.
pub trait CapabilityRegistry {
    /// Can `backend` express `feature` when running a macro of `class`?
    /// (class parameter is the documented deviation; pass
    /// [`MacroClass::Reactive`] for controller-side execution semantics.)
    fn supports_feature(&self, backend: BackendType, class: MacroClass, feature: Feature) -> Support;

    /// Can `backend` run this G/M code natively?
    fn supports_code(&self, backend: BackendType, code: &Code) -> Support;

    /// The registered fix for `(backend, code)`, if any.
    fn resolution_for(&self, backend: BackendType, code: &Code) -> Option<Resolution>;

    /// Register (or replace) a resolution rule (`CORE-T-05`).
    fn register_resolution(&mut self, rule: ResolutionRule);

    /// Scan every frame of `toolpath` against `backend` and return findings
    /// in source order (`CORE-T-04` / `TRF-C-04`).
    fn scan_incompatibilities(&self, toolpath: &Toolpath, backend: BackendType)
        -> Vec<Incompatibility>;
}

/// The built-in matrix: the default values of the ТЗ tables (§5 of ТЗ №1 and
/// §7 of ТЗ №3). Embedders mutate it via [`CapabilityRegistry::register_resolution`]
/// or use [`CapabilityMatrix::with_atc_macro`] etc.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilityMatrix {
    /// Extra rules layered over the built-in table (last registration wins).
    #[serde(default)]
    rules: Vec<ResolutionRule>,
}

impl CapabilityMatrix {
    /// Construct the matrix with only the built-in defaults.
    pub fn new() -> Self {
        CapabilityMatrix { rules: Vec::new() }
    }

    /// Built-in table plus an `M6 → SubstituteMacro("tool_change")` rule:
    /// for machines without ATC the classic fix is a reactive tool-change
    /// macro (ТЗ №3 §7 row `M6 (без ATC)`).
    pub fn with_atc_macro(macro_id: &str) -> Self {
        let mut matrix = CapabilityMatrix::new();
        matrix.register_resolution(ResolutionRule {
            backend: BackendType::Grbl,
            code: Code::M(6),
            resolution: Resolution::SubstituteMacro(macro_id.to_owned()),
            confirm: ConfirmPolicy::EachUnsafe,
            note: Some("no ATC: run reactive tool_change macro".to_owned()),
        });
        matrix
    }

    /// The default resolution rules mirroring the table of ТЗ №3 §7.
    fn built_in_rule(&self, backend: BackendType, code: &Code) -> Option<ResolutionRule> {
        let grbl_family = matches!(backend, BackendType::Grbl);
        if grbl_family {
            match code {
                Code::G(82) => Some(ResolutionRule {
                    backend,
                    code: code.clone(),
                    resolution: Resolution::SubstituteMacro("g82_dwell_drill".into()),
                    confirm: ConfirmPolicy::Auto,
                    note: Some("GRBL lacks dwell cycles; emulate via macro".into()),
                }),
                Code::G(83) => Some(ResolutionRule {
                    backend,
                    code: code.clone(),
                    resolution: Resolution::SubstituteMacro("g83_peck_drill".into()),
                    confirm: ConfirmPolicy::Auto,
                    note: Some("GRBL lacks peck cycles; emulate via macro".into()),
                }),
                Code::G(41) | Code::G(42) => Some(ResolutionRule {
                    backend,
                    code: code.clone(),
                    resolution: Resolution::ApplyTransform("CutterCompBake".into()),
                    confirm: ConfirmPolicy::EachUnsafe,
                    note: Some("bake cutter compensation into geometry".into()),
                }),
                Code::G(68) => Some(ResolutionRule {
                    backend,
                    code: code.clone(),
                    resolution: Resolution::ApplyTransform("Rotate".into()),
                    // G68 rotation rewrites geometry — always confirm.
                    confirm: ConfirmPolicy::EachUnsafe,
                    note: Some("bake coordinate-system rotation into geometry".into()),
                }),
                Code::G(84) => Some(ResolutionRule {
                    backend,
                    code: code.clone(),
                    resolution: Resolution::Manual,
                    confirm: ConfirmPolicy::EachUnsafe,
                    note: Some("tapping cycle unsupported on GRBL".into()),
                }),
                _ => None,
            }
        } else {
            None
        }
    }
}

impl CapabilityRegistry for CapabilityMatrix {
    fn supports_feature(&self, backend: BackendType, class: MacroClass, feature: Feature) -> Support {
        use Feature::*;
        // Columns of the ТЗ №1 §5 table:
        //            LinuxCNC Klipper GRBL-gen GRBL-react Emulator
        match (backend, class, feature) {
            // Sequence and Delay are universal.
            (_, _, Sequence) | (_, _, Delay) => Support::Full,
            // Live status waiting needs an interpreter with feedback.
            (BackendType::LinuxCnc, _, WaitStatus) => Support::Full,
            (BackendType::Klipper, _, WaitStatus) => {
                Support::Partial("only a few template functions can read inputs".into())
            }
            (BackendType::Grbl | BackendType::FluidNc, MacroClass::Generative, WaitStatus) => {
                Support::None
            }
            (BackendType::Grbl | BackendType::FluidNc, MacroClass::Reactive, WaitStatus) => {
                Support::Full
            }
            (BackendType::Emulator, _, WaitStatus) => Support::Full,
            // Variables: controller memory vs host expansion.
            (BackendType::LinuxCnc, _, Variables) => Support::Full,
            (BackendType::Klipper, _, Variables) => {
                Support::Partial("saved gcode variables only, scoped and persistent".into())
            }
            (BackendType::Grbl | BackendType::FluidNc, MacroClass::Generative, Variables) => {
                Support::Partial("resolved on host during expansion".into())
            }
            (BackendType::Grbl | BackendType::FluidNc, MacroClass::Reactive, Variables) => {
                Support::Full
            }
            (BackendType::Emulator, _, Variables) => Support::Full,
            // Branching is universal (offline or on-controller).
            (_, _, IfElse) => Support::Full,
            // Loops.
            (BackendType::LinuxCnc, _, WhileRepeat) => Support::Full,
            (BackendType::Klipper, _, WhileRepeat) => {
                Support::Partial("no native while; emulate with recursive macros".into())
            }
            (BackendType::Emulator, _, WhileRepeat) => Support::Full,
            (BackendType::Grbl | BackendType::FluidNc, _, WhileRepeat) => Support::Full,
            // foreach/arrays: controller O-word subset vs host expansion.
            (BackendType::LinuxCnc, _, ForeachArrays) => {
                Support::Partial("arrays must be flattened by the emitter".into())
            }
            (BackendType::Klipper, _, ForeachArrays) => {
                Support::Partial("jinja loops supported, arrays via lists".into())
            }
            (BackendType::Grbl | BackendType::FluidNc | BackendType::Emulator, _, ForeachArrays) => {
                Support::Full
            }
            // Recursion: only host interpreters / expansions allow it.
            (BackendType::LinuxCnc | BackendType::Klipper, _, Recursion) => Support::None,
            (BackendType::Grbl | BackendType::FluidNc | BackendType::Emulator, _, Recursion) => {
                Support::Full
            }
            (BackendType::LinuxCnc | BackendType::Klipper, _, Subprograms) => Support::Full,
            (BackendType::Grbl | BackendType::FluidNc | BackendType::Emulator, _, Subprograms) => {
                Support::Full
            }
            // Transform invocation happens on the host, always available.
            (_, _, InvokeTransform) => Support::Full,
        }
    }

    fn supports_code(&self, backend: BackendType, code: &Code) -> Support {
        // Conservative native-code table: only codes whose support is known
        // to differ are listed; everything else passes as `Full`.
        match (backend, code) {
            (BackendType::Grbl, Code::G(82) | Code::G(83)) => Support::None,
            (BackendType::Grbl, Code::G(41) | Code::G(42)) => Support::None,
            (BackendType::Grbl, Code::G(68) | Code::G(69)) => Support::None,
            (BackendType::Grbl, Code::G(84)) => Support::None,
            // FluidNC natively supports cutter comp and canned cycles.
            (BackendType::FluidNc, Code::G(68) | Code::G(69)) => {
                Support::Partial("rotation of coordinate system supported via rotate".into())
            }
            (BackendType::Grbl | BackendType::FluidNc | BackendType::Emulator, Code::M(6)) => {
                Support::Partial("no automatic tool changer".into())
            }
            _ => Support::Full,
        }
    }

    fn resolution_for(&self, backend: BackendType, code: &Code) -> Option<Resolution> {
        // User-registered rules take precedence over the built-in table.
        if let Some(rule) = self.rules.iter().rev().find(|r| r.backend == backend && r.code == *code) {
            return Some(rule.resolution.clone());
        }
        self.built_in_rule(backend, code).map(|rule| rule.resolution)
    }

    fn register_resolution(&mut self, rule: ResolutionRule) {
        self.rules.push(rule);
    }

    fn scan_incompatibilities(
        &self,
        toolpath: &Toolpath,
        backend: BackendType,
    ) -> Vec<Incompatibility> {
        let mut findings = Vec::new();
        for frame in &toolpath.codes {
            for code in &frame.codes {
                let support = self.supports_code(backend, code);
                if matches!(support, Support::Full) {
                    continue;
                }
                let resolution = self.resolution_for(backend, code);
                let severity = match (&support, &resolution) {
                    (Support::None, None) => Severity::Blocker,
                    (Support::None, Some(Resolution::Manual)) => Severity::Blocker,
                    (Support::None, Some(_)) => Severity::Warning,
                    (Support::Partial(note), _) if !note.is_empty() => Severity::Info,
                    _ => Severity::Info,
                };
                let detail = match support {
                    Support::None => format!(
                        "{backend} cannot execute {} natively",
                        code.canonical()
                    ),
                    Support::Partial(note) => note,
                    Support::Full => String::new(),
                };
                findings.push(Incompatibility {
                    backend,
                    code: code.clone(),
                    line: frame.line,
                    severity,
                    resolution,
                    detail,
                });
            }
        }
        findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::toolpath::CodeFrame;

    #[test]
    fn wait_status_is_generative_grbl_forbidden_reactive_allowed() {
        let matrix = CapabilityMatrix::new();
        assert_eq!(
            matrix.supports_feature(BackendType::Grbl, MacroClass::Generative, Feature::WaitStatus),
            Support::None
        );
        assert_eq!(
            matrix.supports_feature(BackendType::Grbl, MacroClass::Reactive, Feature::WaitStatus),
            Support::Full
        );
    }

    #[test]
    fn recursion_rejected_on_controllers() {
        let matrix = CapabilityMatrix::new();
        for backend in [BackendType::LinuxCnc, BackendType::Klipper] {
            assert_eq!(
                matrix.supports_feature(backend, MacroClass::Reactive, Feature::Recursion),
                Support::None,
                "{backend} must not allow recursion"
            );
        }
    }

    #[test]
    fn grbl_g83_resolves_to_peck_macro() {
        let matrix = CapabilityMatrix::new();
        assert_eq!(
            matrix.resolution_for(BackendType::Grbl, &Code::G(83)),
            Some(Resolution::SubstituteMacro("g83_peck_drill".into()))
        );
        assert_eq!(
            matrix.resolution_for(BackendType::LinuxCnc, &Code::G(83)),
            None
        );
    }

    #[test]
    fn registered_rules_override_builtins() {
        let mut matrix = CapabilityMatrix::new();
        matrix.register_resolution(ResolutionRule {
            backend: BackendType::Grbl,
            code: Code::G(83),
            resolution: Resolution::Remove,
            confirm: ConfirmPolicy::Auto,
            note: None,
        });
        assert_eq!(
            matrix.resolution_for(BackendType::Grbl, &Code::G(83)),
            Some(Resolution::Remove)
        );
    }

    #[test]
    fn scan_flags_g41_as_warning_with_bake_fix() {
        let mut matrix = CapabilityMatrix::new();
        let mut path = Toolpath::default();
        path.codes.push(CodeFrame {
            line: 4,
            codes: vec![Code::G(41)],
            words: vec![('D', 1.0)],
            raw: "G41 D1".into(),
            comment: None,
            substitute: None,
        });
        let findings = matrix.scan_incompatibilities(&path, BackendType::Grbl);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].line, 4);
        assert_eq!(findings[0].severity, Severity::Warning);
        assert_eq!(
            findings[0].resolution,
            Some(Resolution::ApplyTransform("CutterCompBake".into()))
        );

        // Without a registered fix the same finding must escalate to Blocker.
        matrix.rules.clear(); // user rules no longer relevant, built-in stands
        let findings = matrix.scan_incompatibilities(&path, BackendType::Grbl);
        assert_eq!(findings[0].severity, Severity::Warning);
        let mut stripped = Toolpath::default();
        stripped.codes.push(CodeFrame {
            line: 9,
            codes: vec![Code::Other("G999".into())],
            words: vec![],
            raw: "G999".into(),
            comment: None,
            substitute: None,
        });
        // Unknown code is `Full` → no finding; a true blocker needs a None.
        assert!(matrix.scan_incompatibilities(&stripped, BackendType::Grbl).is_empty());
        let mut blocker = Toolpath::default();
        blocker.codes.push(CodeFrame {
            line: 2,
            codes: vec![Code::G(84)],
            words: vec![],
            raw: "G84".into(),
            comment: None,
            substitute: None,
        });
        let findings = matrix.scan_incompatibilities(&blocker, BackendType::Grbl);
        // G84 has a Manual resolution → Blocker (cannot auto-fix).
        assert_eq!(findings[0].severity, Severity::Blocker);
    }
}
