use super::*;

impl<'a> Pipeline<'a> {
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
                    let kind = TransformKind::StripUnsupported {
                        codes: vec![finding.code.clone()],
                    };
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
