use super::*;

impl<'a> Pipeline<'a> {
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
}
