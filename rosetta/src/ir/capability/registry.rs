use serde::{Deserialize, Serialize};

use crate::ir::macro_ir::MacroClass;
use crate::ir::toolpath::{Code, Toolpath};

use super::model::{
    BackendType, ConfirmPolicy, Feature, Incompatibility, Resolution, ResolutionRule, Severity,
    Support,
};

/// The unified read interface both engines depend on (`CORE-M-03`,
/// `CORE-T-03/04`). Objects implementing this trait are the only sanctioned
/// source of capability answers; keeping it a trait lets embedders swap in a
/// machine-specific profile (e.g. GRBL with/without ATC) without changing
/// engine code.
pub trait CapabilityRegistry {
    /// Can `backend` express `feature` when running a macro of `class`?
    /// (class parameter is the documented deviation; pass
    /// [`MacroClass::Reactive`] for controller-side execution semantics.)
    fn supports_feature(
        &self,
        backend: BackendType,
        class: MacroClass,
        feature: Feature,
    ) -> Support;

    /// Can `backend` run this G/M code natively?
    fn supports_code(&self, backend: BackendType, code: &Code) -> Support;

    /// The registered fix for `(backend, code)`, if any.
    fn resolution_for(&self, backend: BackendType, code: &Code) -> Option<Resolution>;

    /// Register (or replace) a resolution rule (`CORE-T-05`).
    fn register_resolution(&mut self, rule: ResolutionRule);

    /// Scan every frame of `toolpath` against `backend` and return findings
    /// in source order (`CORE-T-04` / `TRF-C-04`).
    fn scan_incompatibilities(
        &self,
        toolpath: &Toolpath,
        backend: BackendType,
    ) -> Vec<Incompatibility>;
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
    fn supports_feature(
        &self,
        backend: BackendType,
        class: MacroClass,
        feature: Feature,
    ) -> Support {
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
            (
                BackendType::Grbl | BackendType::FluidNc | BackendType::Emulator,
                _,
                ForeachArrays,
            ) => Support::Full,
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
        // Preserve dialect codes in the parser, but never treat an unrecognized
        // G-code as executable just because no backend-specific rule mentions it.
        let is_g_code = match code {
            Code::G(_) => true,
            Code::Other(token) => token.starts_with('G'),
            Code::M(_) => false,
        };
        if is_g_code && !code.is_recognized_g_code() {
            return Support::None;
        }
        if let Code::M(number) = code {
            let common_m_code = matches!(*number, 0..=9 | 30 | 98 | 99 | 100..=199);
            if !common_m_code {
                return Support::None;
            }
        }
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
        if let Some(rule) = self
            .rules
            .iter()
            .rev()
            .find(|r| r.backend == backend && r.code == *code)
        {
            return Some(rule.resolution.clone());
        }
        self.built_in_rule(backend, code)
            .map(|rule| rule.resolution)
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
                    Support::None => {
                        format!("{backend} cannot execute {} natively", code.canonical())
                    }
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
            matrix.supports_feature(
                BackendType::Grbl,
                MacroClass::Generative,
                Feature::WaitStatus
            ),
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
        let mut unknown = Toolpath::default();
        unknown.codes.push(CodeFrame {
            line: 9,
            codes: vec![Code::G(8768)],
            words: vec![],
            raw: "G8768".into(),
            comment: None,
            substitute: None,
        });
        // Unknown numeric G-codes are blockers rather than silently executable passthrough.
        let findings = matrix.scan_incompatibilities(&unknown, BackendType::Grbl);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].code, Code::G(8768));
        assert_eq!(findings[0].severity, Severity::Blocker);
        assert_eq!(findings[0].line, 9);
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
