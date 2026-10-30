//! End-to-end scenarios over the assembled Rosetta system — the acceptance
//! stories of the three ТЗ documents wired together (ТЗ №1 §8, ТЗ №2 §11,
//! ТЗ №3 §9.3/§12).
//!
//! The unit tests prove the parts; these prove the *composition contracts*:
//! `MAC-T-*` (macro → transform bridge) and `TRF-M-*` (transform → macro
//! substitution), plus the meta-operation "make this program runnable on
//! this controller" through the shared capability registry.

use std::collections::HashMap;

use rosetta::io::gcode::GcodeIo;
use rosetta::io::{MacroFrontend, ToolpathIo, YamlFrontend};
use rosetta::ir::capability::{BackendType, CapabilityMatrix, CapabilityRegistry, Resolution};
use rosetta::ir::macro_ir::Value;
use rosetta::ir::toolpath::Code;
use rosetta::macro_engine::interpreter::TransformHook;
use rosetta::macro_engine::runtime::{
    CancellationToken, DryRunRuntime, MachineState, MacroLogLine, RunOutcome, ScriptedRuntime,
};
use rosetta::macro_engine::{
    find_trigger, generative, safety, MacroInvoker, MacroRegistry, TriggerEvent,
};
use rosetta::transform::affine::AffineTransform;
use rosetta::transform::model::{
    CycleCode, MacroExpander, ProgramRef, Region, Transform, TransformKind, TransformStep,
};
use rosetta::transform::pipeline::{Pipeline, PlanConfirmation};
use rosetta::transform::registry::TransformRegistry;

/// The composition story of ТЗ №3 §9.3: a reactive probing macro measures
/// the stock angle, then calls `Rotate(angle, cx, cy)` through the
/// `InvokeTransform` bridge; on GRBL the rotate is baked into coordinates.
#[test]
fn measure_stock_macro_bakes_rotation_into_toolpath() {
    // 1) Author the macro exactly in the storage YAML (serde_yaml_ng tags).
    let macro_yaml = "meta: { id: measure_stock, name: Measure stock, class: reactive }\n\
        locals:\n  - { name: angle, kind: number }\n\
        body:\n\
        \x20 - !gcode { template: \"G90 G0 Z5\" }\n\
        \x20 - !set_var { name: angle, value: !status probe_angle }\n\
        \x20 - !invoke_transform { id: Rotate, args: [ !var angle, !num 120.0, !num 40.0 ] }\n";
    let program = YamlFrontend.parse_macro(macro_yaml).unwrap();

    let mut registry = MacroRegistry::new();
    registry.register(program.clone()).unwrap();
    let catalog = TransformRegistry::with_builtins();

    // 2) The host runs the macro against a canned probe readout; the
    //    transform bridge decodes Rotate through the catalog and rotates an
    //    actual toolpath captured by the host.
    let mut status = HashMap::new();
    status.insert("probe_angle".to_owned(), Value::Num(90.0));
    let mut runtime = ScriptedRuntime::new(status, MachineState::Idle);
    let cancel = CancellationToken::new();
    let mut events: Vec<MacroLogLine> = Vec::new();

    // Geometry the bridge should rotate: a horizontal edge ending at (20,0).
    let mut toolpath = GcodeIo::new().parse("G0 X10 Y0\nG1 X20 Y0\n").unwrap();
    let mut apply_rotate = |id: &str, args: Vec<Value>| -> rosetta::RosettaResult<Option<Value>> {
        assert_eq!(id, "Rotate");
        let kind = catalog.build_from_args(id, &args)?;
        let transform = AffineTransform::new(kind);
        let report = transform.apply(&mut toolpath, &Region::Whole)?;
        Ok(Some(Value::Num(report.affected_segments as f64)))
    };
    let mut holder: Option<&mut TransformHook<'_>> = Some(&mut apply_rotate);
    let outcome = rosetta::macro_engine::interpreter::run_macro(
        &program,
        Vec::new(),
        &mut runtime,
        &cancel,
        &mut |line| events.push(line),
        &mut holder,
    )
    .unwrap();
    // The bridge's return value is surfaced via the "return" convention.
    assert!(
        matches!(outcome, RunOutcome::Completed { value: Some(Value::Num(n)) } if n == 2.0),
        "{outcome:?}"
    );
    // The macro sent its own motion, read the probe, and asked for the bake.
    assert_eq!(runtime.sent.first().map(String::as_str), Some("G90 G0 Z5"));
    assert!(events.iter().any(|e| e.message.contains("transform Rotate")));
    // Rotate 90° CCW about (120, 40): (10,0) → (160, -70), (20,0) → (160, -60).
    let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
    let first = toolpath.segments[0].to;
    assert!(close(first.x, 160.0) && close(first.y, -70.0), "{first:?}");
    let second = toolpath.segments[1].to;
    assert!(close(second.x, 160.0) && close(second.y, -60.0), "{second:?}");

    // 3) Dry-run variant (ТЗ №2 §9.4) without a bridge must fail on the
    //    transform step: proof that InvokeTransform is gated, not silently
    //    skipped (principle 4).
    let mut runtime2 = ScriptedRuntime::new(canned_probe(), MachineState::Idle);
    let mut dry = DryRunRuntime::new(&mut runtime2);
    let mut no_bridge: Option<&mut TransformHook<'_>> = None;
    let outcome = rosetta::macro_engine::interpreter::run_macro(
        &program,
        Vec::new(),
        &mut dry,
        &CancellationToken::new(),
        &mut |_| {},
        &mut no_bridge,
    );
    assert!(matches!(
        outcome,
        Err(rosetta::RosettaError::NotFound { ref kind, .. }) if kind == "transform bridge"
    ));
    // The logic up to the failure did run, but only as [DRY RUN] records.
    assert_eq!(dry.recorded, vec!["[DRY RUN] G90 G0 Z5"]);
}

/// Canned probe status map shared by dry-run scenarios.
fn canned_probe() -> HashMap<String, Value> {
    let mut status = HashMap::new();
    status.insert("probe_angle".to_owned(), Value::Num(90.0));
    status
}

/// The meta-operation: scan a GRBL-incompatible program, auto-plan the
/// substitutions through the shared resolution registry, apply, and verify
/// the artifact still connects to the macro catalog (TRF-M-01/02/05/06).
#[test]
fn make_runnable_on_grbl_end_to_end() {
    let source = "G21 G90\nG0 X5 Y5\nG83 Z-10 Q2 F120\nG41 D1\nM6\nG0 X20 Y20\nM30\n";

    // Step 1: what does GRBL actually refuse?
    let matrix = CapabilityMatrix::new();
    let toolpath = GcodeIo::new().parse(source).unwrap();
    let findings = matrix.scan_incompatibilities(&toolpath, BackendType::Grbl);
    let codes: Vec<String> = findings.iter().map(|f| f.code.canonical()).collect();
    assert!(codes.contains(&"G83".to_owned()), "{codes:?}");
    assert!(codes.contains(&"G41".to_owned()), "{codes:?}");
    // The resolution registry answers for each finding:
    assert!(matches!(
        matrix.resolution_for(BackendType::Grbl, &Code::G(83)),
        Some(Resolution::SubstituteMacro(id)) if id == "g83_peck_drill"
    ));
    assert!(matches!(
        matrix.resolution_for(BackendType::Grbl, &Code::G(41)),
        Some(Resolution::ApplyTransform(name)) if name == "CutterCompBake"
    ));

    // Step 2: build the plan. CycleSubstitute is parameter-determined and
    // goes in automatically; CutterCompBake needs the tool radius, so the
    // meta-operation reports it as an open prompt instead of guessing.
    let registry = TransformRegistry::with_builtins();
    let expander = PeckExpander;
    let pipeline = Pipeline::new(&matrix, &registry, Some(&expander), None);
    let (mut plan, unresolved) = pipeline
        .auto_plan(
            source,
            BackendType::Grbl,
            ProgramRef { id: "pockets.nc".into(), label: None },
            "make-runnable",
            "Make runnable on GRBL",
            rosetta::ir::capability::ConfirmPolicy::EachUnsafe,
        )
        .unwrap();
    assert_eq!(plan.steps.len(), 1, "{:?}", plan.steps); // only the G83 swap
    // G41 needs parameters; M6 has no registered rule on the bare matrix.
    assert_eq!(unresolved.len(), 2, "{unresolved:?}");
    assert!(unresolved.iter().any(|u| u.contains("G41")));
    assert!(unresolved.iter().any(|u| u.contains("M6")));

    // Step 3: with G41 removed by an explicit operator decision (strip),
    // the full pipeline produces a traced artifact.
    plan.steps.push(TransformStep::new(TransformKind::StripUnsupported {
        codes: vec![Code::G(41)],
    }));
    let artifact =
        pipeline.run(source, &plan, PlanConfirmation::PlanLevel { approved: true }).unwrap();
    assert!(artifact.emitted_text.contains("g83_peck_drill"), "{}", artifact.emitted_text);
    assert!(!artifact.emitted_text.contains("G41"), "{}", artifact.emitted_text);
    // Tracing: original line 3 (G83) and line 4 (G41) are both accounted.
    let traced: Vec<usize> =
        artifact.traces.iter().flat_map(|t| t.affected_lines.clone()).collect();
    assert!(traced.contains(&3) && traced.contains(&4), "{traced:?}");
    assert_eq!(artifact.source_text, source);

    // Step 4 (TRF-M-01/02): the substituted macro must exist in the engine's
    // registry for the *runtime* substitution path too, and expand offline.
    let mut macros = MacroRegistry::new();
    let peck = YamlFrontend
        .parse_macro(
            "meta: { id: g83_peck_drill, name: Peck drill, class: generative }\n\
             params:\n  - { name: z, default: !num -1.0 }\n\
             body:\n\
             \x20 - !foreach { var: i, iter: !range { start: !num 0.0, end: !num 1.0 },\n\
             \x20     body: [ !gcode { template: \"G1 Z{z}\" } ] }\n",
        )
        .unwrap();
    macros.register(peck.clone()).unwrap();
    // Capability gate for the target backend (MAC-C-07 / CORE-M-03):
    let lines = generative::expand_program(&peck, &matrix, BackendType::Grbl).unwrap();
    assert_eq!(lines, vec!["G1 Z-1"]);
}

/// Minimal stand-in for the application's macro library expander: produces
/// canned peck lines so the pipeline stage is exercised without the engine.
struct PeckExpander;

impl MacroExpander for PeckExpander {
    fn expand(
        &self,
        macro_id: &str,
        cycle: CycleCode,
        frame: &rosetta::ir::toolpath::CodeFrame,
    ) -> rosetta::RosettaResult<Vec<String>> {
        let z = frame.words.iter().find(|(l, _)| *l == 'Z').map(|(_, v)| *v).unwrap_or(0.0);
        Ok(vec![format!("({macro_id} emulates {cycle:?})"), format!("G1 Z{z}")])
    }
}

/// Trigger routing end-to-end (ТЗ №2 §4): an M6 event finds the tool-change
/// macro, its confirm flag travels with the trigger, and safety prestart
/// refuses while the controller is in ALARM (§9.2 / MAC-C-07).
#[test]
fn mcode_trigger_routes_tool_change_with_confirm_and_alarm_gate() {
    let macro_yaml = "meta: { id: tool_change, name: Tool change, class: reactive }\n\
        triggers:\n\
        \x20 - { type: mcode, code: 6, require_confirm: true }\n\
        \x20 - { type: button, require_confirm: true }\n\
        body:\n  - !gcode { template: \"M0 (operator: change T1)\" }\n";
    let program = YamlFrontend.parse_macro(macro_yaml).unwrap();
    let mut registry = MacroRegistry::new();
    registry.register(program.clone()).unwrap();

    // The controller emits M6 while streaming…
    let matched = find_trigger(&program.triggers, &TriggerEvent::MCode(6));
    assert!(matched.is_some(), "MAC-C-05 routing must hit the trigger");
    let report = safety::prestart(&program, matched, MachineState::Idle);
    assert!(report.allowed && report.requires_confirm, "{report:?}");

    // …but the operator has thrown ALARM meanwhile (§9.2).
    let report = safety::prestart(&program, matched, MachineState::Alarm);
    assert!(!report.allowed, "ALARM must block the start");

    // The execute() path refuses against an alarm runtime, too, proving the
    // gate lives inside the sanctioned entry point (MAC-C-07).
    let mut runtime = ScriptedRuntime::new(HashMap::new(), MachineState::Alarm);
    let mut holder: Option<&mut TransformHook<'_>> = None;
    let matrix = CapabilityMatrix::new();
    let error = safety::execute(
        &program,
        Vec::new(),
        &mut runtime,
        &CancellationToken::new(),
        &mut |_| {},
        &mut holder,
        &matrix,
        BackendType::Grbl,
    )
    .unwrap_err();
    assert!(matches!(error, rosetta::RosettaError::Runtime { .. }), "{error}");
    assert!(runtime.sent.is_empty(), "nothing may reach the machine");
}

/// The Invoker entry point resolves ids through the registry and honors the
/// class routing table (MAC-C-04): generative programs expand to flat code,
/// and unknown ids surface as NotFound (MAC-T-02).
#[test]
fn invoker_routes_by_class_and_registry_is_the_source_of_truth() {
    let mut registry = MacroRegistry::new();
    let generative_src = "meta: { id: bolt_circle, name: Bolt circle, class: generative }\n\
        params:\n  - { name: r, default: !num 2.0 }\n\
        body:\n  - !repeat { count: !num 2.0, body: [ !gcode { template: \"G1 X{r}\" } ] }\n";
    registry.register(YamlFrontend.parse_macro(generative_src).unwrap()).unwrap();

    let matrix = CapabilityMatrix::new();
    let id = "bolt_circle".to_owned();
    let program = registry.get(&id).unwrap().clone();
    let lines = generative::expand_program(&program, &matrix, BackendType::FluidNc).unwrap();
    assert_eq!(lines, vec!["G1 X2", "G1 X2"]);

    // Unknown id => NotFound through the trait entry point (MAC-T-02).
    let mut invoker =
        rosetta::macro_engine::Invoker::new(&registry, &matrix, BackendType::Grbl, None);
    let mut runtime = ScriptedRuntime::new(HashMap::new(), MachineState::Idle);
    let mut log = |_l: MacroLogLine| {};
    let missing = "missing_id".to_owned();
    let error = invoker
        .invoke(&missing, Vec::new(), &mut runtime, &CancellationToken::new(), &mut log)
        .unwrap_err();
    assert!(
        matches!(error, rosetta::RosettaError::NotFound { ref kind, ref id }
            if kind == "macro" && id == "missing_id")
    );
}
