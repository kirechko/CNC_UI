//! Error type shared by every Rosetta subsystem.
//!
//! The specifications demand "explicit degradation" (ТЗ №1 principle 4):
//! unsupported constructs must fail with a clear, located message instead of
//! silently distorting the program. [`RosettaError`] variants encode exactly
//! the information needed for such messages: the source line for parse errors,
//! the offending backend + construct for capability rejections, and the
//! roadmap phase for features that are deliberately not implemented yet.

use std::fmt;

/**
 * The single error type of the Rosetta system (`RosettaResult<T>` is its
 * `Result` alias). Sized and `Clone` so reports can be stored in artifacts.
 */
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RosettaError {
    /// A source (G-code or YAML) could not be parsed. `line` is 1-based when known.
    Parse { line: usize, reason: String },
    /// A document/plan/argument failed semantic validation (types, duplicates).
    Validation { reason: String },
    /// The target backend cannot express a construct (capability check, `CORE-M-03`).
    Capability { backend: String, construct: String, detail: String },
    /// A referenced object (macro, subroutine, transform) does not exist.
    NotFound { kind: String, id: String },
    /// Feature intentionally not implemented yet; `phase` names the roadmap step.
    Unsupported { what: String, phase: String },
    /// The controller/host reported a failure while executing a step.
    Runtime { reason: String },
    /// A macro explicitly aborted itself (`Stmt::Abort`).
    Abort { reason: String },
    /// A geometric operation is impossible (e.g. arc under non-uniform scale).
    Geometry { reason: String },
    /// Underlying I/O failure (file access for configs/artifacts).
    Io { reason: String },
}

pub type RosettaResult<T> = Result<T, RosettaError>;

impl fmt::Display for RosettaError {
    /// Renders every variant as a compact single-line message suitable for UI
    /// log panels; the structured fields stay available for programmatic use.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RosettaError::Parse { line, reason } => {
                if *line == 0 {
                    write!(f, "parse error: {reason}")
                } else {
                    write!(f, "parse error at line {line}: {reason}")
                }
            }
            RosettaError::Validation { reason } => write!(f, "validation error: {reason}"),
            RosettaError::Capability { backend, construct, detail } => {
                write!(f, "{backend} cannot express {construct}: {detail}")
            }
            RosettaError::NotFound { kind, id } => write!(f, "{kind} \"{id}\" not found"),
            RosettaError::Unsupported { what, phase } => {
                write!(f, "{what} is not implemented yet (roadmap phase {phase})")
            }
            RosettaError::Runtime { reason } => write!(f, "runtime error: {reason}"),
            RosettaError::Abort { reason } => write!(f, "macro aborted: {reason}"),
            RosettaError::Geometry { reason } => write!(f, "geometry error: {reason}"),
            RosettaError::Io { reason } => write!(f, "I/O error: {reason}"),
        }
    }
}

impl std::error::Error for RosettaError {}

impl From<std::io::Error> for RosettaError {
    /// Maps filesystem failures into the common `Io` variant.
    fn from(error: std::io::Error) -> Self {
        RosettaError::Io { reason: error.to_string() }
    }
}

impl From<serde_yaml_ng::Error> for RosettaError {
    /// Maps YAML (de)serialization failures into `Parse` with line 0, because
    /// serde_yaml line positions are already inside its message text.
    fn from(error: serde_yaml_ng::Error) -> Self {
        RosettaError::Parse { line: 0, reason: error.to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_error_shows_location_and_reason() {
        let error = RosettaError::Parse { line: 7, reason: "unknown word".into() };
        assert_eq!(error.to_string(), "parse error at line 7: unknown word");
        let unlocated = RosettaError::Parse { line: 0, reason: "bad yaml".into() };
        assert_eq!(unlocated.to_string(), "parse error: bad yaml");
    }

    #[test]
    fn unsupported_error_names_the_roadmap_phase() {
        let error = RosettaError::Unsupported {
            what: "O-words frontend".into(),
            phase: "2-3".into(),
        };
        assert!(error.to_string().contains("roadmap phase 2-3"));
    }
}
