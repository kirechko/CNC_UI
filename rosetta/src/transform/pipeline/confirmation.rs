use super::*;

impl<'a> Pipeline<'a> {
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
}
