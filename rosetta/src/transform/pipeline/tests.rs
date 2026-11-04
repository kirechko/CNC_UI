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
        let z = frame
            .words
            .iter()
            .find(|(l, _)| *l == 'Z')
            .map(|(_, v)| *v)
            .unwrap_or(0.0);
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
    let g83: Vec<_> = findings
        .iter()
        .filter(|f| f.code == crate::ir::toolpath::Code::G(83))
        .collect();
    assert_eq!(g83.len(), 2);
    assert!(g83
        .iter()
        .all(|f| matches!(f.resolution, Some(Resolution::SubstituteMacro(_)))));
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
            ProgramRef {
                id: "drill.nc".into(),
                label: None,
            },
            "fix-grbl",
            "Make runnable on GRBL",
            ConfirmPolicy::EachUnsafe,
        )
        .unwrap();
    assert!(unresolved.is_empty(), "{unresolved:?}");
    assert_eq!(plan.steps.len(), 2);
    // Cycle substitution is non-confirmable by default (auto rules), so
    // `Auto` passes the gate.
    let artifact = pipe
        .run(GRBL_CYCLE_PROGRAM, &plan, PlanConfirmation::Auto)
        .unwrap();
    assert!(artifact
        .emitted_text
        .contains("(g83_peck_drill: peck to -10)"));
    assert!(artifact.emitted_text.contains("G1 Z-10 F200"));
    // TRF-M-06: the trace references the ORIGINAL frame lines (4 and 6).
    let substituted: Vec<_> = artifact
        .traces
        .iter()
        .flat_map(|t| t.affected_lines.clone())
        .collect();
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
        source_ref: ProgramRef {
            id: "p".into(),
            label: None,
        },
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
    assert!(
        artifact.emitted_text.contains("X58.660254"),
        "{:?}",
        artifact.emitted_text
    );
    assert!(
        artifact.emitted_text.contains("Y55"),
        "{:?}",
        artifact.emitted_text
    );
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
        source_ref: ProgramRef {
            id: "p".into(),
            label: None,
        },
        target_backend: BackendType::Grbl,
        steps: vec![TransformStep::new(TransformKind::Rotate {
            angle_deg: 180.0,
            center: Point2::new(0.0, 0.0),
        })],
        confirm_policy: ConfirmPolicy::Auto,
    };
    let error = pipe
        .run("G0 X50 Y50\nG1 X60 Y50\n", &plan, PlanConfirmation::Auto)
        .unwrap_err();
    assert!(matches!(error, RosettaError::Geometry { .. }), "{error}");

    // Region-limited rotate breaks the motion chain → validate note.
    let mut tp = pipe.parse("G0 X10 Y0\nG1 X20 Y0\n").unwrap();
    let region_plan = TransformPlan {
        id: "part".into(),
        name: "partial".into(),
        source_ref: ProgramRef {
            id: "p".into(),
            label: None,
        },
        target_backend: BackendType::Grbl,
        steps: vec![TransformStep {
            kind: TransformKind::Rotate {
                angle_deg: 90.0,
                center: Point2::new(0.0, 0.0),
            },
            region: crate::transform::model::Region::Lines { from: 2, to: 2 },
            requires_confirm: false,
            reversible: true,
            note: None,
        }],
        confirm_policy: ConfirmPolicy::Auto,
    };
    let traces = pipe.apply(&mut tp, &region_plan).unwrap();
    let notes = pipe.validate(&tp, &region_plan, &traces).unwrap();
    assert!(
        notes.iter().any(|n| n.contains("path discontinuity")),
        "{notes:?}"
    );
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
        source_ref: ProgramRef {
            id: "p".into(),
            label: None,
        },
        target_backend: BackendType::Grbl,
        steps: Vec::new(),
        confirm_policy: ConfirmPolicy::Auto,
    };
    let error = pipe
        .run("G21\nG84 Z-5\n", &plan, PlanConfirmation::Auto)
        .unwrap_err();
    assert!(matches!(error, RosettaError::Capability { .. }), "{error}");
}
