//! Offline expansion of **generative** macros into flat G-code lines
//! (`CORE-M-04`, consumed by `io::FlatEmitter`, ТЗ №2 §5 "генеративный"
//! routing to GRBL/FluidNC/emulator).
//!
//! The idea: reuse the *same* interpreter as the reactive path, but drive it
//! with [`EmitRuntime`] — a [`HostRuntime`] that writes to a line buffer
//! instead of a machine. Because that runtime refuses `status_field` reads
//! and reports `Idle` forever, any reactive-only construct fails expansion
//! with an explicit capability/`Unsupported`-style error rather than
//! producing silently wrong code (ТЗ №1 principle 4).

use crate::error::{RosettaError, RosettaResult};
use crate::ir::capability::{BackendType, CapabilityMatrix, CapabilityRegistry, Support};
use crate::ir::macro_ir::{MacroClass, MacroProgram};
use crate::macro_engine::interpreter;
use crate::macro_engine::runtime::{CancellationToken, EmitRuntime, RunOutcome};
use crate::macro_engine::safety;

/// Expand `program` into flat lines ready to stream to `backend`.
///
/// Steps:
/// 1. Structural validation (`CORE-M-02`) so malformed IR never reaches the
///    interpreter.
/// 2. Capability gate for `(backend, MacroClass::Generative)` over the
///    features the program actually uses (`CORE-M-03` / `MAC-C-03`): a
///    generative macro that waits on live status, for example, is *not
///    expressible* on GRBL and must be rejected here, not at runtime.
/// 3. Deterministic execution against [`EmitRuntime`]: variables resolve,
///    loops unroll, subroutines inline, `Delay` materializes as `G4`.
///
/// Fails with `Validation` if the program aborts or is cancelled mid
/// expansion — offline expansion has no side effects to roll back, but it
/// must not emit partial code either; the caller gets `Err` and emits
/// nothing.
pub fn expand_program(
    program: &MacroProgram,
    matrix: &CapabilityMatrix,
    backend: BackendType,
) -> RosettaResult<Vec<String>> {
    program.validate()?;

    // Gate 2: every construct the program uses must be expressible by the
    // backend in the generative mode (offline).
    for feature in safety::scan_features(program) {
        let support = matrix.supports_feature(backend, MacroClass::Generative, feature);
        if !support.supported() {
            let detail = match &support {
                Support::Partial(why) => why.clone(),
                Support::None => "construct cannot be expanded offline".to_owned(),
                Support::Full => unreachable!("supported() checked above"),
            };
            return Err(RosettaError::Capability {
                backend: backend.label().to_owned(),
                construct: format!("{feature:?}"),
                detail,
            });
        }
    }

    // Gate 3: run the interpreter in emit mode. No live machine, no
    // cancellation source, no transform bridge (InvokeTransform needs a host
    // and is meaningless for pure expansion on this path).
    let mut emit = EmitRuntime::new();
    let cancel = CancellationToken::new();
    let outcome = interpreter::run_macro(program, Vec::new(), &mut emit, &cancel, &mut |_| {}, &mut None)?;
    match outcome {
        RunOutcome::Completed { .. } => Ok(emit.lines),
        RunOutcome::Aborted { reason } => Err(RosettaError::Validation {
            reason: format!("generative expansion aborted at analysis time: {reason}"),
        }),
        RunOutcome::Cancelled => Err(RosettaError::Validation {
            reason: "generative expansion was cancelled".to_owned(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::{MacroFrontend, YamlFrontend};

    /// Generative macro parsed from spec-shaped YAML (serde_yaml_ng tagged
    /// form): one interpolated line plus an unrolled loop plus an inline
    /// subroutine call.
    const SOURCE: &str = "meta: { id: gen, name: Gen, class: generative }\n\
        params:\n  - { name: depth, default: !num -2.0 }\n\
        subs:\n  - name: lead\n    body:\n      - !gcode { template: \"G0 Z0.5\" }\n\
        body:\n\
        \x20 - !call { name: lead }\n\
        \x20 - !gcode { template: \"G1 Z{depth} F100\" }\n\
        \x20 - !repeat { count: !num 3.0, body: [ !gcode { template: \"G4 P0.1\" } ] }\n";

    #[test]
    fn expands_variables_subs_and_loops() {
        let program = YamlFrontend.parse_macro(SOURCE).unwrap();
        let lines = expand_program(&program, &CapabilityMatrix::new(), BackendType::Grbl).unwrap();
        assert_eq!(
            lines,
            vec!["G0 Z0.5", "G1 Z-2 F100", "G4 P0.1", "G4 P0.1", "G4 P0.1"]
        );
    }

    #[test]
    fn expansion_refuses_status_waits_for_generative_grbl() {
        // CORE-M-03 pre-flight: Wait is reactive-only on GRBL.
        let program = YamlFrontend
            .parse_macro(
                "meta: { id: gen2, name: Gen2, class: generative }\n\
                 body:\n  - !wait { cond: !status probe_trig }\n",
            )
            .unwrap();
        let error = expand_program(&program, &CapabilityMatrix::new(), BackendType::Grbl).unwrap_err();
        assert!(matches!(error, RosettaError::Capability { ref construct, .. } if construct == "WaitStatus"));
    }

    #[test]
    fn delay_materializes_as_dwell_line() {
        let program = YamlFrontend
            .parse_macro(
                "meta: { id: gen3, name: Gen3, class: generative }\n\
                 body:\n  - !delay { ms: !num 500.0 }\n",
            )
            .unwrap();
        let lines = expand_program(&program, &CapabilityMatrix::new(), BackendType::Emulator).unwrap();
        assert_eq!(lines, vec!["G4 P500"]);
    }
}
