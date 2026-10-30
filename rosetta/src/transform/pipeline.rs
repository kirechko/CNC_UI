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
use crate::ir::capability::{
    BackendType, CapabilityMatrix, CapabilityRegistry, ConfirmPolicy, Incompatibility, Resolution,
    Severity,
};
use crate::ir::toolpath::Toolpath;
use crate::io::gcode::GcodeIo;
use crate::io::ToolpathIo;
use crate::transform::model::{
    Artifact, CycleCode, Limits, MacroExpander, ProgramRef, TraceEntry, TransformKind,
    TransformPlan, TransformReport, TransformStep,
};
use crate::transform::registry::TransformRegistry;
use crate::transform::structural;

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
        Pipeline { io: GcodeIo::new(), matrix, registry, expander, limits }
    }

    /// Stage *Load→Parse* (`TRF-C-02`).
    pub fn parse(&self, source: &str) -> RosettaResult<Toolpath> {
        self.io.parse(source)
    }

    /// Stage *Detect* (`TRF-C-04`): findings straight from the core scanner.
    pub fn detect(&self, toolpath: &Toolpath, backend: BackendType) -> Vec<Incompatibility> {
        self.matrix.scan_incompatibilities(toolpath, backend)
    }

    /// Stage *Plan* (ТЗ №3 §9.3 meta-operation "make this program runnable"):
    /// turn every fixable finding into a concrete step where the resolution
    /// fully determines the parameters; everything else (Rotate angle, tool
    /// radius — open question §15) is returned as an actionable prompt for
    /// the UI instead of a guessed step.
    pub fn suggest(&self, findings: &[Incompatibility]) -> (Vec<TransformStep>, Vec<String>) {
        let mut steps = Vec::new();
        let mut unresolved = Vec::new();
        for finding in findings {
            match &finding.resolution {
                Some(Resolution::SubstituteMacro(macro_id)) => {
                    match CycleCode::from_code(&finding.code) {
                        Some(cycle) => {
                            let kind = TransformKind::CycleSubstitute {
                                from: cycle,
                                to: macro_id.clone(),
                            };
                            steps.push(TransformStep::new(kind));
                        }
                        None => unresolved.push(format!(
                            "{}: macro \"{macro_id}\" substitution needs a manual step",
                            finding.code
                        )),
                    }
                }
                Some(Resolution::ApplyTransform(name)) => {
                    match self.registry.build_from_args(name, &[]) {
                        Ok(kind) => steps.push(TransformStep::new(kind)),
                        Err(_) => unresolved.push(format!(
                            "{}: transform \"{name}\" requires parameters (angle / radius)",
                            finding.code
                        )),
                    }
                }
                Some(Resolution::Remove) => {
                    let kind = TransformKind::StripUnsupported { codes: vec![finding.code.clone()] };
                    steps.push(TransformStep::new(kind));
                }
                Some(Resolution::Manual) | None => unresolved.push(format!(
                    "{}: {}",
                    finding.code,
                    if finding.severity == Severity::Blocker {
                        "blocker without a registered resolution"
                    } else {
                        "manual attention suggested"
                    }
                )),
            }
        }
        (steps, unresolved)
    }

    /// Stage *Confirm* (§11.2): the hard gate mapping [`ConfirmPolicy`] and
    /// [`PlanConfirmation`] into a go/no-go decision.
    pub fn gate(plan: &TransformPlan, confirmation: PlanConfirmation<'_>) -> RosettaResult<()> {
        let needs: Vec<usize> = plan
            .steps
            .iter()
            .enumerate()
            .filter(|(_, step)| step.requires_confirm)
            .map(|(index, _)| index)
            .collect();
        if needs.is_empty() || plan.confirm_policy == ConfirmPolicy::Auto {
            return Ok(());
        }
        match confirmation {
            PlanConfirmation::Auto => Err(RosettaError::Validation {
                reason: format!(
                    "plan \"{}\" has {} confirmable step(s); operator approval required (policy {:?})",
                    plan.id,
                    needs.len(),
                    plan.confirm_policy
                ),
            }),
            PlanConfirmation::PlanLevel { approved } => {
                if approved {
                    Ok(())
                } else {
                    Err(RosettaError::Validation {
                        reason: "plan approval declined by the operator".into(),
                    })
                }
            }
            PlanConfirmation::EachUnsafe { approvals } => {
                let missing: Vec<usize> =
                    needs.iter().copied().filter(|i| !approvals.contains(i)).collect();
                if missing.is_empty() {
                    Ok(())
                } else {
                    Err(RosettaError::Validation {
                        reason: format!("steps {missing:?} require confirmation before apply"),
                    })
                }
            }
        }
    }

    /// Stage *Apply* (`TRF-C-01/06`): run every step through its executor,
    /// collect per-step traces keyed by *original* source lines.
    pub fn apply(
        &self,
        toolpath: &mut Toolpath,
        plan: &TransformPlan,
    ) -> RosettaResult<Vec<TraceEntry>> {
        let mut traces = Vec::new();
        for (index, step) in plan.steps.iter().enumerate() {
            if !self.registry.exists(step.kind.name()) {
                return Err(RosettaError::NotFound {
                    kind: "transform".into(),
                    id: step.kind.name().to_owned(),
                });
            }
            let executor = structural::executor(&step.kind, self.expander);
            let report: TransformReport = executor.apply(toolpath, &step.region)?;
            let note = match (&step.note, report.notes.is_empty()) {
                (Some(text), true) => Some(text.clone()),
                (None, true) => None,
                (Some(text), false) => Some(format!("{text}; {}", report.notes.join("; "))),
                (None, false) => Some(report.notes.join("; ")),
            };
            traces.push(TraceEntry {
                step_index: index,
                name: step.kind.name().to_owned(),
                note,
                affected_lines: report.affected_lines,
            });
        }
        Ok(traces)
    }

    /// Stage *Validate* (§11.3). Hard errors: out-of-limit points, broken
    /// path continuity, leftover Blocker findings. Warnings come back as
    /// notes strings for the UI (soft-limit proximity is not defined here —
    /// limits are hard bounds by model).
    pub fn validate(
        &self,
        toolpath: &Toolpath,
        plan: &TransformPlan,
        traces: &[TraceEntry],
    ) -> RosettaResult<Vec<String>> {
        let mut notes = Vec::new();
        if let Some(limits) = &self.limits {
            for segment in &toolpath.segments {
                for point in [&segment.from, &segment.to] {
                    if !limits.contains(point) {
                        return Err(RosettaError::Geometry {
                            reason: format!(
                                "line {}: point ({}, {}, {}) outside soft limits [{x}..{X}]/[{y}..{Y}]/[{z}..{Z}]",
                                segment.line,
                                crate::util::fmt_num(point.x),
                                crate::util::fmt_num(point.y),
                                crate::util::fmt_num(point.z),
                                x = limits.x_min,
                                X = limits.x_max,
                                y = limits.y_min,
                                Y = limits.y_max,
                                z = limits.z_min,
                                Z = limits.z_max,
                            ),
                        });
                    }
                }
            }
        }
        // Path integrity: the parser produced segments that chain exactly
        // (to == next.from); a region-limited transform can break that.
        for pair in toolpath.segments.windows(2) {
            let (previous, next) = (&pair[0], &pair[1]);
            if previous.kind == crate::ir::toolpath::SegmentKind::Dwell
                || next.kind == crate::ir::toolpath::SegmentKind::Dwell
            {
                continue; // dwells are zero-length, not motion-chain links
            }
            if !crate::util::almost_equal(previous.to.x, next.from.x, 1e-6)
                || !crate::util::almost_equal(previous.to.y, next.from.y, 1e-6)
                || !crate::util::almost_equal(previous.to.z, next.from.z, 1e-6)
            {
                notes.push(format!(
                    "path discontinuity before line {} (previous motion ended at ({}, {}, {}))",
                    next.line,
                    crate::util::fmt_num(previous.to.x),
                    crate::util::fmt_num(previous.to.y),
                    crate::util::fmt_num(previous.to.z),
                ));
            }
        }
        // Re-scan: a plan that leaves blockers is not finished (§11.3 in
        // spirit "dry-run before release"; the matrix scan is the offline
        // equivalent we can run here).
        let remaining = self.detect(toolpath, plan.target_backend);
        let blockers: Vec<String> = remaining
            .iter()
            .filter(|f| f.severity == Severity::Blocker)
            .map(|f| format!("{} at line {}", f.code, f.line))
            .collect();
        if !blockers.is_empty() {
            return Err(RosettaError::Capability {
                backend: plan.target_backend.label().to_owned(),
                construct: blockers.join(", "),
                detail: format!(
                    "plan \"{}\" still leaves {} blocker(s) unresolved",
                    plan.id,
                    blockers.len()
                ),
            });
        }
        // TRF-M-06 provenance check: every cycle substitution must trace.
        for frame in &toolpath.codes {
            if frame.substitute.is_some()
                && !traces
                    .iter()
                    .any(|t| t.name == "CycleSubstitute" && t.affected_lines.contains(&frame.line))
            {
                return Err(RosettaError::Validation {
                    reason: format!(
                        "line {} was substituted but the artifact has no matching trace entry",
                        frame.line
                    ),
                });
            }
        }
        Ok(notes)
    }

    /// Stages *Emit → Artifact* (principles 3): the finished program with
    /// verbatim source and step traces.
    pub fn artifact(
        &self,
        plan: &TransformPlan,
        source: &str,
        toolpath: &Toolpath,
        traces: Vec<TraceEntry>,
        findings: Vec<Incompatibility>,
    ) -> RosettaResult<Artifact> {
        let emitted_text = self.io.emit(toolpath)?;
        Ok(Artifact {
            plan_id: plan.id.clone(),
            target_backend: plan.target_backend,
            source_text: source.to_owned(),
            emitted_text,
            traces,
            findings,
        })
    }

    /// Full one-call run of the §5 pipeline. `confirmation` is enforced as
    /// the hard gate *before* any mutation.
    pub fn run(
        &self,
        source: &str,
        plan: &TransformPlan,
        confirmation: PlanConfirmation<'_>,
    ) -> RosettaResult<Artifact> {
        let mut toolpath = self.parse(source)?;
        let findings = self.detect(&toolpath, plan.target_backend);
        Self::gate(plan, confirmation)?;
        let traces = self.apply(&mut toolpath, plan)?;
        self.validate(&toolpath, plan, &traces)?;
        self.artifact(plan, source, &toolpath, traces, findings)
    }

    /// Build a ready-to-gate plan skeleton from a source document: detects
    /// findings, suggests steps, marks the plan unsafe-by-default so the UI
    /// only needs to collect approvals.
    pub fn auto_plan(
        &self,
        source: &str,
        backend: BackendType,
        source_ref: ProgramRef,
        id: &str,
        name: &str,
        policy: ConfirmPolicy,
    ) -> RosettaResult<(TransformPlan, Vec<String>)> {
        let toolpath = self.parse(source)?;
        let findings = self.detect(&toolpath, backend);
        let (steps, unresolved) = self.suggest(&findings);
        let plan = TransformPlan {
            id: id.to_owned(),
            name: name.to_owned(),
            source_ref,
            target_backend: backend,
            steps,
            confirm_policy: policy,
        };
        Ok((plan, unresolved))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::capability::{BackendType, CapabilityMatrix, ConfirmPolicy};
    use crate::ir::toolpath::Point2;
    use crate::transform::registry::TransformRegistry;

    struct Canned;
    impl MacroExpander for Canned {
        fn expand(
            &self,
            macro_id: &str,
            _cycle: CycleCode,
            frame: &crate::ir::toolpath::CodeFrame,
        ) -> RosettaResult<Vec<String>> {
            let z = frame.words.iter().find(|(l, _)| *l == 'Z').map(|(_, v)| *v).unwrap_or(0.0);
            Ok(vec![
                format!("({macro_id}: peck to {z})"),
                format!("G1 Z{z} F200"),
                "G0 Z0.5".into(),
            ])
        }
    }

    fn pipeline<'a>(matrix: &'a CapabilityMatrix, registry: &'a TransformRegistry) -> Pipeline<'a> {
        Pipeline::new(matrix, registry, Some(&Canned), None)
    }

    const GRBL_CYCLE_PROGRAM: &str =
        "G21\nG90\nG0 X5 Y5\nG83 Z-10 Q2 F100\nG0 X20 Y5\nG83 Z-8 Q2 F100\nG80\nM30\n";

    #[test]
    fn detect_finds_g83_as_fixable_warning_on_grbl() {
        let matrix = CapabilityMatrix::new();
        // Bind the registry: the pipeline borrows it for its whole lifetime.
        let registry = TransformRegistry::with_builtins();
        let pipe = pipeline(&matrix, &registry);
        let tp = pipe.parse(GRBL_CYCLE_PROGRAM).unwrap();
        let findings = pipe.detect(&tp, BackendType::Grbl);
        let g83: Vec<_> = findings.iter().filter(|f| f.code == crate::ir::toolpath::Code::G(83)).collect();
        assert_eq!(g83.len(), 2);
        assert!(g83.iter().all(|f| matches!(f.resolution, Some(Resolution::SubstituteMacro(_)))));
    }

    #[test]
    fn auto_plan_substitutes_cycles_and_run_emits_traced_artifact() {
        let matrix = CapabilityMatrix::new();
        let registry = TransformRegistry::with_builtins();
        let pipe = pipeline(&matrix, &registry);
        let (plan, unresolved) = pipe
            .auto_plan(
                GRBL_CYCLE_PROGRAM,
                BackendType::Grbl,
                ProgramRef { id: "drill.nc".into(), label: None },
                "fix-grbl",
                "Make runnable on GRBL",
                ConfirmPolicy::EachUnsafe,
            )
            .unwrap();
        assert!(unresolved.is_empty(), "{unresolved:?}");
        assert_eq!(plan.steps.len(), 2);
        // Cycle substitution is non-confirmable by default (auto rules), so
        // `Auto` passes the gate.
        let artifact = pipe.run(GRBL_CYCLE_PROGRAM, &plan, PlanConfirmation::Auto).unwrap();
        assert!(artifact.emitted_text.contains("(g83_peck_drill: peck to -10)"));
        assert!(artifact.emitted_text.contains("G1 Z-10 F200"));
        // TRF-M-06: the trace references the ORIGINAL frame lines (4 and 6).
        let substituted: Vec<_> = artifact.traces.iter().flat_map(|t| t.affected_lines.clone()).collect();
        assert_eq!(substituted, vec![4, 6]);
        // Artifact keeps the verbatim source for the before/after UI.
        assert_eq!(artifact.source_text, GRBL_CYCLE_PROGRAM);
    }

    #[test]
    fn gate_blocks_unapproved_geometry_steps() {
        let matrix = CapabilityMatrix::new();
        let registry = TransformRegistry::with_builtins();
        let pipe = Pipeline::new(&matrix, &registry, None, None);
        let mut plan = TransformPlan {
            id: "rotate".into(),
            name: "Rotate stock".into(),
            source_ref: ProgramRef { id: "p".into(), label: None },
            target_backend: BackendType::Grbl,
            steps: vec![TransformStep::new(TransformKind::Rotate {
                angle_deg: 30.0,
                center: Point2::new(50.0, 50.0),
            })],
            confirm_policy: ConfirmPolicy::EachUnsafe,
        };
        plan.steps[0].requires_confirm = true;
        // Auto without approval: refused.
        assert!(Pipeline::gate(&plan, PlanConfirmation::Auto).is_err());
        // Explicit per-step approval: allowed.
        Pipeline::gate(&plan, PlanConfirmation::EachUnsafe { approvals: &[0] }).unwrap();
        // Blanket approval also passes (UI "accept all").
        Pipeline::gate(&plan, PlanConfirmation::PlanLevel { approved: true }).unwrap();
        // ...but only when actually approved.
        assert!(Pipeline::gate(&plan, PlanConfirmation::PlanLevel { approved: false }).is_err());
        // And the run applies the rotation under approval.
        let artifact = pipe
            .run(
                "G0 X50 Y50\nG1 X60 Y50\n",
                &plan,
                PlanConfirmation::EachUnsafe { approvals: &[0] },
            )
            .unwrap();
        // (60,50) rotated 30° CCW around (50,50): offset (10,0) →
        // (10·cos30, 10·sin30) = (8.660254, 5) ⇒ X58.660254 Y55.
        assert!(artifact.emitted_text.contains("X58.660254"), "{:?}", artifact.emitted_text);
        assert!(artifact.emitted_text.contains("Y55"), "{:?}", artifact.emitted_text);
    }

    #[test]
    fn limits_reject_rotated_path_and_region_edits_flag_discontinuity() {
        let matrix = CapabilityMatrix::new();
        let registry = TransformRegistry::with_builtins();
        let mut pipe = Pipeline::new(&matrix, &registry, None, None);
        pipe.limits = Some(Limits {
            x_min: 0.0,
            x_max: 100.0,
            y_min: 0.0,
            y_max: 100.0,
            z_min: -50.0,
            z_max: 10.0,
        });
        let plan = TransformPlan {
            id: "far".into(),
            name: "rotate into negative X".into(),
            source_ref: ProgramRef { id: "p".into(), label: None },
            target_backend: BackendType::Grbl,
            steps: vec![TransformStep::new(TransformKind::Rotate {
                angle_deg: 180.0,
                center: Point2::new(0.0, 0.0),
            })],
            confirm_policy: ConfirmPolicy::Auto,
        };
        let error = pipe.run("G0 X50 Y50\nG1 X60 Y50\n", &plan, PlanConfirmation::Auto).unwrap_err();
        assert!(matches!(error, RosettaError::Geometry { .. }), "{error}");

        // Region-limited rotate breaks the motion chain → validate note.
        let mut tp = pipe.parse("G0 X10 Y0\nG1 X20 Y0\n").unwrap();
        let region_plan = TransformPlan {
            id: "part".into(),
            name: "partial".into(),
            source_ref: ProgramRef { id: "p".into(), label: None },
            target_backend: BackendType::Grbl,
            steps: vec![TransformStep {
                kind: TransformKind::Rotate { angle_deg: 90.0, center: Point2::new(0.0, 0.0) },
                region: crate::transform::model::Region::Lines { from: 2, to: 2 },
                requires_confirm: false,
                reversible: true,
                note: None,
            }],
            confirm_policy: ConfirmPolicy::Auto,
        };
        let traces = pipe.apply(&mut tp, &region_plan).unwrap();
        let notes = pipe.validate(&tp, &region_plan, &traces).unwrap();
        assert!(notes.iter().any(|n| n.contains("path discontinuity")), "{notes:?}");
    }

    #[test]
    fn blockers_without_resolution_stop_the_plan() {
        // G84 on GRBL has a Manual resolution → scan flags it Blocker;
        // an empty plan then fails validation on the leftover blocker.
        let matrix = CapabilityMatrix::new();
        let registry = TransformRegistry::with_builtins();
        let pipe = Pipeline::new(&matrix, &registry, None, None);
        let plan = TransformPlan {
            id: "noop".into(),
            name: "no steps".into(),
            source_ref: ProgramRef { id: "p".into(), label: None },
            target_backend: BackendType::Grbl,
            steps: Vec::new(),
            confirm_policy: ConfirmPolicy::Auto,
        };
        let error = pipe.run("G21\nG84 Z-5\n", &plan, PlanConfirmation::Auto).unwrap_err();
        assert!(matches!(error, RosettaError::Capability { .. }), "{error}");
    }
}
