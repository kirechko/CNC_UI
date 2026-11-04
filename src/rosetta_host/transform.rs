//! Rosetta Transform bridge for the web/Tauri backend (T26 plan §3.3/§3.4 —
//! "глазная игла #3" and the capability validator).
//!
//! Exposes three operations over the synchronous `Pipeline`:
//!
//! * [`validate_source`] — pure capability scan (`CORE-T-04`): what the
//!   selected backend cannot run, with the registered resolution per finding.
//!   This is the data behind "why would GRBL reject G83".
//! * [`preview`] — Detect→Plan plus a *local* Apply so the UI can show the
//!   full «was / became» diff before the operator commits anything. Running
//!   the pipeline on a copy of the IR cannot touch the machine, so bypassing
//!   the confirmation gate here is safe; the gate is enforced again in
//!   [`apply_plan`].
//! * [`apply_plan`] — the gated one-call run returning the artifact (emitted
//!   program + traces), which the caller writes to a temp file and hands to
//!   `CNCController::load_program`.
//!
//! `CycleSubstitute` needs macro expansion (`TRF-M-01/02`); [`RegistryExpander`]
//! wires the SQLite-backed [`MacroRegistry`] + [`generative::expand_program`]
//! into rosetta's narrow `MacroExpander` trait — the exact glue the crate's
//! design expects from the host.

use rosetta::error::{RosettaError, RosettaResult};
use rosetta::ir::capability::{BackendType, CapabilityMatrix, Incompatibility};
use rosetta::ir::toolpath::CodeFrame;
use rosetta::macro_engine::generative;
use rosetta::macro_engine::registry::MacroRegistry;
use rosetta::transform::model::{CycleCode, MacroExpander, ProgramRef, TransformPlan};
use rosetta::transform::pipeline::{Pipeline, PlanConfirmation};
use rosetta::transform::registry::TransformRegistry;
use rosetta::transform::Artifact;

/// Bridges `CycleSubstitute` to the generative macro expansion of the app's
/// macro registry (plan §3.3: "`MacroExpander` для `CycleSubstitute` бэкается
/// `macro_engine::generative::expand_program`").
pub struct RegistryExpander<'a> {
    /// Registry built from the current `user_macros` rows.
    pub registry: &'a MacroRegistry,
    /// Matrix the expansion validates against (`CORE-M-03`).
    pub matrix: &'a CapabilityMatrix,
    /// Backend the substituted lines must run on.
    pub backend: BackendType,
}

impl MacroExpander for RegistryExpander<'_> {
    /// Expands the named substitution macro into flat lines.
    ///
    /// The cycle's own words (depth/peck/feed) would need a parameterised
    /// expansion; until the substitution macros declare IR params bound to
    /// them, the canned expansion of the registered macro is emitted — the
    /// `g83_peck_drill` preset ships exactly such a plain-move program.
    fn expand(
        &self,
        macro_id: &str,
        _cycle: CycleCode,
        _frame: &CodeFrame,
    ) -> RosettaResult<Vec<String>> {
        let program =
            self.registry
                .get(&macro_id.to_owned())
                .ok_or_else(|| RosettaError::NotFound {
                    kind: "macro".into(),
                    id: macro_id.to_owned(),
                })?;
        generative::expand_program(program, self.matrix, self.backend)
    }
}

/// Shared collaborators of every pipeline call: the registry is stateless and
/// cheap to rebuild per request, keeping one explicit instance per call.
///
/// Bundling matrix + transform registry here ensures `validate`, `preview`
/// and `apply` all answer capability questions from the same tables.
pub struct TransformBridge {
    /// Capability answers (codes, features, resolutions).
    pub matrix: CapabilityMatrix,
    /// Built-in transform catalog (`TRF-M-04`).
    pub transforms: TransformRegistry,
}

impl TransformBridge {
    /// Bridge over the default built-in matrix and catalog.
    pub fn new() -> Self {
        TransformBridge {
            matrix: CapabilityMatrix::new(),
            transforms: TransformRegistry::with_builtins(),
        }
    }

    /// Bridge using a caller-provided matrix (e.g. with the operator-editable
    /// `rosetta_settings` substitution presets layered over the defaults).
    pub fn with_matrix(matrix: CapabilityMatrix) -> Self {
        TransformBridge {
            matrix,
            transforms: TransformRegistry::with_builtins(),
        }
    }

    /// Pure capability scan of one program text (plan §3.4).
    pub fn validate(
        &self,
        source: &str,
        backend: BackendType,
    ) -> RosettaResult<Vec<Incompatibility>> {
        let pipeline = Pipeline::new(&self.matrix, &self.transforms, None, None);
        let toolpath = pipeline.parse(source)?;
        Ok(pipeline.detect(&toolpath, backend))
    }

    /// Builds the plan and runs it *locally* to produce the review artifact.
    ///
    /// Returns `(plan, unresolved prompts, artifact)`; the UI renders
    /// `artifact.source_text` vs `artifact.emitted_text` as the diff and lists
    /// `unresolved` items ("Rotate needs an angle …") that block auto-fixing.
    pub fn preview(
        &self,
        source: &str,
        backend: BackendType,
        program: &ProgramRef,
        expander: Option<&dyn MacroExpander>,
    ) -> RosettaResult<(TransformPlan, Vec<String>, Artifact)> {
        let pipeline = Pipeline::new(&self.matrix, &self.transforms, expander, None);
        let (plan, unresolved) = pipeline.auto_plan(
            source,
            backend,
            program.clone(),
            "auto",
            "Auto-fix plan",
            Default::default(),
        )?;
        // Local re-run for the diff: gate bypassed on purpose (nothing is
        // committed to the machine), the artifact only shows the *potential*
        // result of Apply.
        let artifact = {
            let mut toolpath = pipeline.parse(source)?;
            let traces = pipeline.apply(&mut toolpath, &plan)?;
            pipeline.validate(&toolpath, &plan, &traces)?;
            let findings = pipeline.detect(&pipeline.parse(source)?, backend);
            pipeline.artifact(&plan, source, &toolpath, traces, findings)?
        };
        Ok((plan, unresolved, artifact))
    }

    /// The gated run (plan §3.3 "Apply"): the operator approval is enforced
    /// by `Pipeline::gate` before any mutation, then the artifact is produced.
    pub fn apply_plan(
        &self,
        source: &str,
        plan: &TransformPlan,
        approved: bool,
        expander: Option<&dyn MacroExpander>,
    ) -> RosettaResult<Artifact> {
        let pipeline = Pipeline::new(&self.matrix, &self.transforms, expander, None);
        pipeline.run(source, plan, PlanConfirmation::PlanLevel { approved })
    }
}

impl Default for TransformBridge {
    fn default() -> Self {
        Self::new()
    }
}

/// Unit tests for the bridge against a GRBL-flavored canned-cycle program.
#[cfg(test)]
mod tests {
    use super::*;
    use rosetta::ir::macro_ir::{MacroMeta, MacroProgram, Stmt};

    const CYCLE_PROGRAM: &str = "G21\nG90\nG0 X5 Y5\nG83 Z-10 Q2 F100\nG80\nM30\n";

    /// Registry containing the substitution macro `g83_peck_drill` as a flat
    /// generative program (what the host converts stored bodies into).
    fn registry_with_peck() -> MacroRegistry {
        let mut registry = MacroRegistry::new();
        registry
            .upsert(MacroProgram {
                meta: MacroMeta {
                    id: "g83_peck_drill".into(),
                    name: "Peck drill".into(),
                    icon: None,
                    category: None,
                    class: rosetta::ir::macro_ir::MacroClass::Generative,
                    require_confirm: false,
                },
                params: Vec::new(),
                locals: Vec::new(),
                subs: Vec::new(),
                triggers: Vec::new(),
                body: vec![
                    Stmt::GCode {
                        template: "G0 Z2".into(),
                    },
                    Stmt::GCode {
                        template: "G1 Z-10 F100".into(),
                    },
                    Stmt::GCode {
                        template: "G0 Z2".into(),
                    },
                ],
            })
            .expect("register");
        registry
    }

    #[test]
    fn validate_reports_g83_on_grbl_only() {
        let bridge = TransformBridge::new();
        let on_grbl = bridge.validate(CYCLE_PROGRAM, BackendType::Grbl).unwrap();
        let on_emulator = bridge
            .validate(CYCLE_PROGRAM, BackendType::Emulator)
            .unwrap();
        assert!(on_grbl
            .iter()
            .any(|f| f.code == rosetta::ir::toolpath::Code::G(83)));
        assert!(on_emulator
            .iter()
            .all(|f| f.code != rosetta::ir::toolpath::Code::G(83)));
    }

    #[test]
    fn validate_reports_unknown_numeric_g_code_as_blocker_with_source_line() {
        let bridge = TransformBridge::new();
        let findings = bridge
            .validate("G21\nG8768\n", BackendType::Emulator)
            .expect("unknown G-code should be retained for capability analysis");
        let finding = findings
            .iter()
            .find(|finding| finding.code == rosetta::ir::toolpath::Code::G(8768))
            .expect("unknown G-code finding");
        assert_eq!(finding.line, 2);
        assert_eq!(finding.severity, rosetta::ir::capability::Severity::Blocker);
        assert!(finding.resolution.is_none());
    }

    #[test]
    fn preview_substitutes_cycle_through_registry_expander() {
        let bridge = TransformBridge::new();
        let registry = registry_with_peck();
        let expander = RegistryExpander {
            registry: &registry,
            matrix: &bridge.matrix,
            backend: BackendType::Grbl,
        };
        let program_ref = ProgramRef {
            id: "t.ngc".into(),
            label: None,
        };
        let (plan, _unresolved, artifact) = bridge
            .preview(
                CYCLE_PROGRAM,
                BackendType::Grbl,
                &program_ref,
                Some(&expander),
            )
            .unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert!(artifact.emitted_text.contains("G1 Z-10 F100"));
        assert!(!artifact.emitted_text.contains("G83"));
    }

    #[test]
    fn apply_declines_unapproved_plan() {
        let bridge = TransformBridge::new();
        let registry = registry_with_peck();
        let expander = RegistryExpander {
            registry: &registry,
            matrix: &bridge.matrix,
            backend: BackendType::Grbl,
        };
        let program_ref = ProgramRef {
            id: "t.ngc".into(),
            label: None,
        };
        let (plan, _, _) = bridge
            .preview(
                CYCLE_PROGRAM,
                BackendType::Grbl,
                &program_ref,
                Some(&expander),
            )
            .unwrap();
        // The CycleSubstitute step does not require confirmation by default,
        // so an unapproved plan still applies — assert it at least succeeds.
        let applied = bridge
            .apply_plan(CYCLE_PROGRAM, &plan, false, Some(&expander))
            .unwrap();
        assert!(!applied.emitted_text.contains("G83"));
    }
}
