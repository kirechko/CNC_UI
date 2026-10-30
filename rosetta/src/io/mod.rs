//! Frontends (sources) and backends (targets) of the Rosetta IR (ТЗ №1 §4).
//!
//! * [`MacroFrontend`] — parses authoring sources into [`MacroProgram`]
//!   (`CORE-M-02`, consumed by `MAC-C-02`). Implemented today by
//!   [`YamlFrontend`] (Phase 1); O-words and DSL frontends are documented
//!   stubs that fail with an explicit `Unsupported` (`Rosetta.md` §4
//!   roadmap Phases 2-3).
//! * [`MacroEmitter`] — renders a [`MacroProgram`] toward a concrete
//!   backend (`CORE-M-04`, consumed by `MAC-C-04`). Implemented today by
//!   [`FlatEmitter`] (offline generative expansion to flat G-code); the
//!   LinuxCNC O-words and Klipper Jinja2 emitters are stubs.
//! * [`ToolpathIo`] — parse/emit contract for the Toolpath IR
//!   (`CORE-T-02`, consumed by `TRF-C-02`), implemented by `gcode::GcodeIo`.
//!
//! Capability validation before emission (`CORE-M-03`) is enforced here:
//! every emitter checks the program against the matrix first and refuses to
//! produce silently-distorted output (ТЗ №1 principle 4).

pub mod gcode;

use serde::{Deserialize, Serialize};

use crate::error::{RosettaError, RosettaResult};
use crate::ir::capability::{BackendType, CapabilityMatrix, CapabilityRegistry, Feature};
use crate::ir::macro_ir::{MacroClass, MacroProgram};

/// Result of emitting one macro toward one backend: the produced program
/// lines plus human-readable notes (e.g. "loop unrolled ×9").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmittedMacro {
    /// Which backend produced this.
    pub backend: BackendType,
    /// Emitted lines without trailing newlines.
    pub lines: Vec<String>,
    /// Optional diagnostics the UI may surface next to the listing.
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Source of macro programs (`CORE-M-02`).
pub trait MacroFrontend {
    /// Parse a source document into the IR, including structural validation.
    fn parse_macro(&self, source: &str) -> RosettaResult<MacroProgram>;
}

/// Target of macro emission (`CORE-M-04`).
pub trait MacroEmitter {
    /// The backend this emitter addresses.
    fn backend(&self) -> BackendType;

    /// Emit `program` toward [`MacroEmitter::backend`]. MUST validate against
    /// the capability matrix first and MUST NOT run anything (`MAC-C-03`).
    fn emit(&self, program: &MacroProgram) -> RosettaResult<EmittedMacro>;
}

/// Trajectory input/output contract (`CORE-T-02`).
pub trait ToolpathIo {
    /// Parse a G-code source into the Toolpath IR.
    fn parse(&self, gcode: &str) -> RosettaResult<crate::ir::toolpath::Toolpath>;

    /// Render a (possibly transformed) toolpath for `backend`.
    fn emit(&self, toolpath: &crate::ir::toolpath::Toolpath) -> RosettaResult<String>;
}

/// The declarative YAML/JSON frontend — the Phase 1 authoring format and the
/// on-disk storage format (ТЗ №2 §3: "хранится всегда как Macro IR"; YAML is
/// merely its serialization).
#[derive(Debug, Default, Clone, Copy)]
pub struct YamlFrontend;

impl MacroFrontend for YamlFrontend {
    /// Deserializes YAML into [`MacroProgram`] and runs the locally
    /// decidable structural checks (`CORE-M-02`). Cross-object reference
    /// validation (subroutines, transform ids) is the registry's job
    /// (`CORE-M-05`).
    fn parse_macro(&self, source: &str) -> RosettaResult<MacroProgram> {
        let program: MacroProgram = serde_yaml_ng::from_str(source)?;
        program.validate()?;
        Ok(program)
    }
}

/// Offline generative expansion to flat G-code for GRBL/FluidNC/emulator
/// (`CORE-M-04`, backend column "GRBL генеративный").
///
/// The emitter performs the deterministic half of macro execution: variable
/// resolution, loop unrolling, subroutine inlining — and then hands the flat
/// line list to the engine for streaming. Any construct the target backend
/// cannot express aborts emission with a capability error naming the first
/// offending feature.
#[derive(Debug, Default, Clone)]
pub struct FlatEmitter {
    /// Capability matrix consulted before expansion (`CORE-M-03`).
    matrix: CapabilityMatrix,
    /// Which backend the flat code targets.
    backend: BackendType,
}

impl FlatEmitter {
    /// Emitter producing flat code for `backend`, validated against `matrix`.
    pub fn new(matrix: CapabilityMatrix, backend: BackendType) -> Self {
        FlatEmitter { matrix, backend }
    }

    /// The matrix this emitter validates against.
    pub fn matrix(&self) -> &CapabilityMatrix {
        &self.matrix
    }
}

impl MacroEmitter for FlatEmitter {
    fn backend(&self) -> BackendType {
        self.backend
    }

    /// Validate + expand. A `reactive` program is rejected outright here:
    /// routing by class is the engine's decision (`MAC-C-04` guarantees the
    /// emitter is only asked to expand generative macros).
    fn emit(&self, program: &MacroProgram) -> RosettaResult<EmittedMacro> {
        if program.meta.class != MacroClass::Generative {
            return Err(RosettaError::Validation {
                reason: format!(
                    "flat emission expects a generative macro, got class {:?} for \"{}\"",
                    program.meta.class, program.meta.id
                ),
            });
        }
        // CORE-M-03: refuse before touching any feature we cannot express.
        for feature in [Feature::Sequence, Feature::Delay, Feature::Variables, Feature::IfElse] {
            if !self.matrix.supports_feature(self.backend, MacroClass::Generative, feature).supported() {
                return Err(RosettaError::Capability {
                    backend: self.backend.label().to_owned(),
                    construct: format!("{feature:?}"),
                    detail: "required by flat expansion".to_owned(),
                });
            }
        }
        // The offline expander lives next to the interpreter so both share
        // one expression evaluator (`macro_engine::generative`).
        let lines = crate::macro_engine::generative::expand_program(
            program,
            &self.matrix,
            self.backend,
        )?;
        Ok(EmittedMacro { backend: self.backend, lines, notes: Vec::new() })
    }
}

/// Deferred Phase 2-3 backend: IR → LinuxCNC O-words subroutines. Present so
/// the trait surface and the roadmap are visible in the API; calling it fails
/// explicitly instead of producing partial output (principle 4).
#[derive(Debug, Default, Clone, Copy)]
pub struct LinuxCncOWordsEmitter;

impl MacroEmitter for LinuxCncOWordsEmitter {
    fn backend(&self) -> BackendType {
        BackendType::LinuxCnc
    }

    fn emit(&self, _program: &MacroProgram) -> RosettaResult<EmittedMacro> {
        Err(RosettaError::Unsupported {
            what: "IR → O-words emitter".into(),
            phase: "2".into(),
        })
    }
}

/// Deferred Phase 2 backend: IR → Klipper `gcode_macro` (Jinja2).
#[derive(Debug, Default, Clone, Copy)]
pub struct KlipperJinjaEmitter;

impl MacroEmitter for KlipperJinjaEmitter {
    fn backend(&self) -> BackendType {
        BackendType::Klipper
    }

    fn emit(&self, _program: &MacroProgram) -> RosettaResult<EmittedMacro> {
        Err(RosettaError::Unsupported {
            what: "IR → Klipper Jinja2 emitter".into(),
            phase: "2".into(),
        })
    }
}

/// Deferred Phase 2-3 frontend: LinuxCNC O-words → IR import.
#[derive(Debug, Default, Clone, Copy)]
pub struct OWordsFrontend;

impl MacroFrontend for OWordsFrontend {
    fn parse_macro(&self, _source: &str) -> RosettaResult<MacroProgram> {
        Err(RosettaError::Unsupported {
            what: "O-words frontend".into(),
            phase: "2-3".into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // `MacroFrontend` is reached through the explicit `crate::io::YamlFrontend`
    // path in the fixture below, so no trait import alias is needed here.

    /// A minimal generative macro used across io/engine tests. Written in
    /// the serde_yaml_ng tagged shape (`!gcode {..}`, `!num 5.0`), which is
    /// the YAML serialization of the externally tagged `Stmt`/`Expr`.
    fn sample_program(class: MacroClass) -> MacroProgram {
        let text = format!(
            "meta: {{ id: m, name: M, class: {} }}\n\
             params:\n  - {{ name: dx, default: !num 5.0 }}\n\
             body:\n\
             \x20 - !gcode {{ template: \"G0 X{{dx}}\" }}\n\
             \x20 - !repeat {{ count: !num 2.0, body: [ !gcode {{ template: \"G1 Y1 F300\" }} ] }}\n",
            class.yaml_name()
        );
        crate::io::YamlFrontend
            .parse_macro(&text)
            .unwrap_or_else(|e| panic!("sample must parse: {e}"))
    }

    #[test]
    fn yaml_frontend_parses_and_validates() {
        let program = YamlFrontend
            .parse_macro("meta: { id: x, name: X }\nbody:\n  - !gcode { template: G0 X1 }\n")
            .unwrap();
        assert_eq!(program.meta.id, "x");
        assert!(YamlFrontend.parse_macro("meta: { id: '', name: X }\nbody: []").is_err());
    }

    #[test]
    fn flat_emitter_rejects_reactive_programs() {
        let program = sample_program(MacroClass::Reactive);
        let emitter = FlatEmitter::new(CapabilityMatrix::new(), BackendType::Grbl);
        let error = emitter.emit(&program).unwrap_err();
        assert!(matches!(error, RosettaError::Validation { .. }));
    }

    #[test]
    fn flat_emitter_unrolls_loops_and_interpolates() {
        let program = sample_program(MacroClass::Generative);
        let emitter = FlatEmitter::new(CapabilityMatrix::new(), BackendType::Grbl);
        let out = emitter.emit(&program).unwrap();
        // dx defaults to 5 → one G0 plus two unrolled G1 lines.
        assert_eq!(out.lines, vec!["G0 X5", "G1 Y1 F300", "G1 Y1 F300"]);
    }

    #[test]
    fn deferred_backends_fail_with_phase_label() {
        let program = sample_program(MacroClass::Generative);
        let error = LinuxCncOWordsEmitter.emit(&program).unwrap_err();
        assert!(matches!(error, RosettaError::Unsupported { ref phase, .. } if phase == "2"));
        let error = KlipperJinjaEmitter.emit(&program).unwrap_err();
        assert!(matches!(error, RosettaError::Unsupported { .. }));
        let error = OWordsFrontend.parse_macro("o<1> sub").unwrap_err();
        assert!(matches!(error, RosettaError::Unsupported { ref phase, .. } if phase == "2-3"));
    }
}
