use super::{Expr, TransformId};
use serde::{Deserialize, Serialize};

/// Statement node of the Macro IR (`CORE-M-01`). Every control-flow construct
/// is static data (block vectors), never offsets or labels, which keeps the
/// IR re-emittable to O-words or Jinja2 without re-parsing.
///
/// # Serialization shapes (documented deviation)
///
/// `Stmt`/[`Expr`] are externally tagged. JSON (the spec's second sanctioned
/// storage format) renders them as the familiar map shape
/// `{"gcode": {"template": "G0 X1"}}`. The `serde_yaml_ng` crate does not
/// accept that map shape and instead uses YAML tags, so hand-written YAML
/// looks like:
///
/// ```yaml
/// body:
///   - !gcode { template: "G0 X1" }
///   - !wait { cond: !status probe_trig }
/// ```
///
/// The two forms describe the identical IR; `YamlFrontend` accepts the tag
/// form and JSON frontends would accept the map form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stmt {
    /// G-code line with `{variable}` interpolation (ТЗ №2 §5).
    #[serde(rename = "gcode")]
    GCode {
        template: String,
    },
    Delay {
        ms: Expr,
    },
    /// Block until the host reports `state == "Idle"` (reactive only).
    /// Generative expansion renders it as an explicit `G4` dwell or errors.
    Wait {
        cond: Expr,
    },
    SetVar {
        name: String,
        value: Expr,
    },
    If {
        cond: Expr,
        then: Vec<Stmt>,
        #[serde(default)]
        otherwise: Vec<Stmt>,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
    },
    Repeat {
        count: Expr,
        body: Vec<Stmt>,
    },
    #[serde(rename = "foreach")]
    ForEach {
        var: String,
        iter: Expr,
        body: Vec<Stmt>,
    },
    /// Call a [`Subroutine`] by name.
    Call {
        name: String,
        #[serde(default)]
        args: Vec<Expr>,
    },
    /// Cross-subsystem call into Rosetta Transform (`MAC-T-03`).
    /// `id` is a transform kind name validated against `TRF-M-04`.
    InvokeTransform {
        id: TransformId,
        #[serde(default)]
        args: Vec<Expr>,
    },
    Return {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        value: Option<Expr>,
    },
    Abort {
        reason: Expr,
    },
    Comment {
        text: String,
    },
}
