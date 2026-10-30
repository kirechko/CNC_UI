//! Rosetta Transform data schema (ТЗ №3 §6) — the vocabulary the pipeline,
//! the registry and the host UI all share.
//!
//! Contracts implemented here: `TRF-C-01` (operations work on Toolpath IR),
//! `TRF-C-06` (every mutation keeps source-line tracing), `TRF-M-04`
//! (transform ids stable for `InvokeTransform` link validation).
//!
//! # Serialization
//!
//! [`TransformKind`] is externally tagged with variant names verbatim
//! (`"Rotate"`, `"CutterCompBake"`, …) — exactly the strings the core
//! Resolution Registry stores inside `Resolution::ApplyTransform`, so a rule
//! like `G68 → ApplyTransform("Rotate")` names a real kind without importing
//! this module (`ir::capability` stays leaf-level; principle "no cyclic
//! coupling").

use serde::{Deserialize, Serialize};

use crate::error::{RosettaError, RosettaResult};
use crate::ir::capability::ConfirmPolicy;
use crate::ir::macro_ir::MacroId;
use crate::ir::toolpath::{Code, Point2, Point3, SegmentId, Toolpath, Units};

/// Axis set flipped by [`TransformKind::Mirror`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MirrorAxes {
    /// Flip X (mirror around the YZ plane).
    X,
    /// Flip Y (mirror around the XZ plane).
    Y,
    /// Flip both — a 180° rotation in the XY plane, arcs keep direction.
    Xy,
}

/// Cutter compensation side of a `G41/G42` bake (`TRF` §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompSide {
    /// G41 — compensation to the left of the motion direction.
    Left,
    /// G42 — compensation to the right.
    Right,
}

/// Lead-in/out specification shared by the deferred geometric transforms
/// (ТЗ №3 §4Б). Declared now so `CutterCompBake`/`LeadInOut` signatures are
/// stable and serializable before Phase D lands.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LeadSpec {
    /// `"linear"` or `"arc"` entry motion.
    pub kind: LeadKind,
    /// Tangential length of the lead move, program units.
    pub length: f64,
    /// Feed multiplier applied while the lead runs (0.5 = half feed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feed_factor: Option<f64>,
}

/// Lead motion shape of [`LeadSpec`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LeadKind {
    Linear,
    Arc,
}

/// Work coordinate systems addressable by `CoordSystemRemap` (ТЗ №3 §4В).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordSystem {
    G54,
    G55,
    G56,
    G57,
    G58,
    G59,
}

impl CoordSystem {
    /// The numeric G-code word of the system (`G54` → 54).
    pub fn number(self) -> u16 {
        match self {
            CoordSystem::G54 => 54,
            CoordSystem::G55 => 55,
            CoordSystem::G56 => 56,
            CoordSystem::G57 => 57,
            CoordSystem::G58 => 58,
            CoordSystem::G59 => 59,
        }
    }

    /// As a capability-layer [`Code`].
    pub fn code(self) -> Code {
        Code::G(self.number())
    }
}

/// Drilling/milling canned cycles eligible for macro emulation
/// (`CycleSubstitute`, ТЗ №3 §7 rows GRBL ❌).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CycleCode {
    G73,
    G76,
    G81,
    G82,
    G83,
    G84,
    G85,
    G86,
}

impl CycleCode {
    /// The capability-layer [`Code`] of the cycle.
    pub fn code(self) -> Code {
        match self {
            CycleCode::G73 => Code::G(73),
            CycleCode::G76 => Code::G(76),
            CycleCode::G81 => Code::G(81),
            CycleCode::G82 => Code::G(82),
            CycleCode::G83 => Code::G(83),
            CycleCode::G84 => Code::G(84),
            CycleCode::G85 => Code::G(85),
            CycleCode::G86 => Code::G(86),
        }
    }

    /// Inverse mapping used when a resolution rule names a bare G-code.
    pub fn from_code(code: &Code) -> Option<Self> {
        match code {
            Code::G(73) => Some(CycleCode::G73),
            Code::G(76) => Some(CycleCode::G76),
            Code::G(81) => Some(CycleCode::G81),
            Code::G(82) => Some(CycleCode::G82),
            Code::G(83) => Some(CycleCode::G83),
            Code::G(84) => Some(CycleCode::G84),
            Code::G(85) => Some(CycleCode::G85),
            Code::G(86) => Some(CycleCode::G86),
            _ => None,
        }
    }
}

/// A single transformation operation (`TRF-C-01`). The three families of
/// ТЗ №3 §4 map onto this enum: A = affine, B = geometric, C = structural.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TransformKind {
    // --- А. Affine: pure coordinate recompute, no new geometry ------------
    /// Rotate the region around `center` by `angle_deg` (CCW positive).
    Rotate { angle_deg: f64, center: Point2 },
    /// Shift the region by `delta`.
    Translate { delta: Point3 },
    /// Scale relative to `origin` by per-axis `factors`.
    Scale { factors: Point3, origin: Point3 },
    /// Mirror the region about the named axes.
    Mirror { axes: MirrorAxes },
    /// Rescale all lengths and feeds from `from` to `to` units.
    UnitsConvert { from: Units, to: Units },
    // --- Б. Geometric: create/change geometry — deferred Phase D ----------
    /// Bake active `G41/G42` offset into explicit coordinates.
    CutterCompBake {
        side: CompSide,
        tool_radius: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lead: Option<LeadSpec>,
    },
    /// Insert lead-in/out moves around cutting segments.
    LeadInOut { spec: LeadSpec },
    /// Round sharp corners of the path by `radius`.
    CornerRound { radius: f64 },
    // --- В. Structural: change program organization ------------------------
    /// Multiply every feed by `feed_factor` and spindle speed by
    /// `speed_factor` (ТЗ №3 §4В; "override", not a rewrite of geometry).
    FeedSpeedOverride { feed_factor: f64, speed_factor: f64 },
    /// Rewrite one work coordinate system into another (G54 → G56 …).
    CoordSystemRemap { from: CoordSystem, to: CoordSystem },
    /// Replace a canned cycle with emulating macro lines (`TRF-M-05/06`).
    CycleSubstitute { from: CycleCode, to: MacroId },
    /// Delete unsupported codes entirely (the `Remove` resolution).
    StripUnsupported { codes: Vec<Code> },
}

impl TransformKind {
    /// The stable catalog id (`TRF-M-04`) — matches the serde variant name
    /// and the strings inside `Resolution::ApplyTransform`.
    pub fn name(&self) -> &'static str {
        match self {
            TransformKind::Rotate { .. } => "Rotate",
            TransformKind::Translate { .. } => "Translate",
            TransformKind::Scale { .. } => "Scale",
            TransformKind::Mirror { .. } => "Mirror",
            TransformKind::UnitsConvert { .. } => "UnitsConvert",
            TransformKind::CutterCompBake { .. } => "CutterCompBake",
            TransformKind::LeadInOut { .. } => "LeadInOut",
            TransformKind::CornerRound { .. } => "CornerRound",
            TransformKind::FeedSpeedOverride { .. } => "FeedSpeedOverride",
            TransformKind::CoordSystemRemap { .. } => "CoordSystemRemap",
            TransformKind::CycleSubstitute { .. } => "CycleSubstitute",
            TransformKind::StripUnsupported { .. } => "StripUnsupported",
        }
    }

    /// Family of the operation (ТЗ №3 §4 A/B/В), drives UI grouping and the
    /// confirmation defaults.
    pub fn family(&self) -> TransformFamily {
        match self {
            TransformKind::Rotate { .. }
            | TransformKind::Translate { .. }
            | TransformKind::Scale { .. }
            | TransformKind::Mirror { .. }
            | TransformKind::UnitsConvert { .. } => TransformFamily::Affine,
            TransformKind::CutterCompBake { .. }
            | TransformKind::LeadInOut { .. }
            | TransformKind::CornerRound { .. } => TransformFamily::Geometric,
            _ => TransformFamily::Structural,
        }
    }

    /// Principle 2 of ТЗ №3: anything that moves metal-bearing geometry must
    /// be confirmed by the operator; pure organizational rewrites do not.
    pub fn default_requires_confirm(&self) -> bool {
        match self {
            TransformKind::FeedSpeedOverride { .. }
            | TransformKind::CycleSubstitute { .. }
            | TransformKind::CoordSystemRemap { .. } => false,
            _ => true,
        }
    }

    /// Principle 5: deterministic rewrites (affine, structural) are
    /// reversible in the sense that the original program is kept in the
    /// artifact; bake-like geometric ops destroy information.
    pub fn default_reversible(&self) -> bool {
        !matches!(
            self,
            TransformKind::CutterCompBake { .. } | TransformKind::LeadInOut { .. } | TransformKind::CornerRound { .. }
        )
    }
}

/// The three operation families of ТЗ №3 §4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformFamily {
    Affine,
    Geometric,
    Structural,
}

/// Which part of the program an operation touches (ТЗ №3 §3 "Регион").
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Region {
    /// The entire program.
    Whole,
    /// Inclusive source-line span, 1-based.
    Lines { from: usize, to: usize },
    /// Between two comment markers, e.g. `;(BEGIN OP2)` … `;(END OP2)`.
    /// Accepted by the schema but rejected by the Phase 1 engine until the
    /// marker standard is decided (ТЗ №3 §15 open question).
    BetweenMarks { start: String, end: String },
    /// Explicit segment ids.
    Segments(Vec<SegmentId>),
}

/// One ordered operation of a plan (`TRF-C-06` traceability unit).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformStep {
    pub kind: TransformKind,
    #[serde(default = "Region::whole")]
    pub region: Region,
    pub requires_confirm: bool,
    pub reversible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

impl Region {
    /// Serde default factory: [`Region::Whole`].
    fn whole() -> Region {
        Region::Whole
    }

    /// Convenience constructor mirroring `TransformStep::default_region`.
    pub fn whole_default() -> Self {
        Region::Whole
    }
}

impl TransformStep {
    /// Build a step with the family-derived defaults of confirmation and
    /// reversibility (`default_requires_confirm`/`default_reversible`).
    pub fn new(kind: TransformKind) -> Self {
        TransformStep {
            requires_confirm: kind.default_requires_confirm(),
            reversible: kind.default_reversible(),
            kind,
            region: Region::Whole,
            note: None,
        }
    }
}

/// Reference to the program a plan was built for (artifact bookkeeping).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramRef {
    /// Stable id, typically the file path or UI buffer id.
    pub id: String,
    /// Human-readable label shown next to the plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// An ordered, confirmable plan of steps (`TRF-C-01`, ТЗ №3 §3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformPlan {
    pub id: String,
    pub name: String,
    pub source_ref: ProgramRef,
    pub target_backend: crate::ir::capability::BackendType,
    pub steps: Vec<TransformStep>,
    #[serde(default)]
    pub confirm_policy: ConfirmPolicy,
}

/// Per-step result of applying one [`TransformKind`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TransformReport {
    /// Number of segments rewritten (0 for pure frame edits).
    pub affected_segments: usize,
    /// Source line numbers the operation touched (artifact tracing).
    pub affected_lines: Vec<usize>,
    /// Diagnostics for the UI ("arc direction flipped", "2 dwell moved", …).
    pub notes: Vec<String>,
}

/// A concrete operation; synchronous deviation from the ТЗ async sketch.
pub trait Transform {
    /// Catalog name for logs and traces.
    fn name(&self) -> &'static str;

    /// Rewrite `toolpath` in place within `region` (`TRF-C-06`: must keep
    /// every touched source line identifiable through the report).
    fn apply(&self, toolpath: &mut Toolpath, region: &Region) -> RosettaResult<TransformReport>;
}

/// Bridge the host supplies so `CycleSubstitute` can materialize macro
/// lines (`TRF-M-01/02`). The transform engine deliberately does not import
/// the macro engine; the application wires `MacroRegistry + CapabilityMatrix
/// + generative::expand_program` into this tiny trait (or runs the reactive
/// macro through the `MacroInvoker` and embeds its emitted preamble).
pub trait MacroExpander {
    /// Expand the substitution of `cycle` found on `frame` into concrete
    /// G-code lines ready for [`CodeFrame::substitute`] storage.
    fn expand(
        &self,
        macro_id: &str,
        cycle: CycleCode,
        frame: &crate::ir::toolpath::CodeFrame,
    ) -> RosettaResult<Vec<String>>;
}

/// One row of the artifact trace: what ran, when, over which original lines
/// (`TRF-M-06`: substitution lines keep the *original* frame line number).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceEntry {
    /// Index into [`TransformPlan::steps`].
    pub step_index: usize,
    /// Catalog id of the kind that ran.
    pub name: String,
    /// Step note or generated diagnostics, joined for readability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Source lines affected by this step.
    pub affected_lines: Vec<usize>,
}

/// The finished program plus everything needed to review or reproduce it
/// (ТЗ №3 principle 3: artifact, not silent overwrite).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Artifact {
    pub plan_id: String,
    pub target_backend: crate::ir::capability::BackendType,
    /// Verbatim source the pipeline was run against.
    pub source_text: String,
    /// Emitted transformed program.
    pub emitted_text: String,
    /// Step-by-step provenance in plan order.
    pub traces: Vec<TraceEntry>,
    /// Operator-visible incompatibility scan the plan was built from.
    #[serde(default)]
    pub findings: Vec<crate::ir::capability::Incompatibility>,
}

/// Soft travel limits the validation stage checks (ТЗ №3 §11.3).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Limits {
    pub x_min: f64,
    pub x_max: f64,
    pub y_min: f64,
    pub y_max: f64,
    pub z_min: f64,
    pub z_max: f64,
}

impl Limits {
    /// Whether `point` lies inside every axis bound.
    pub fn contains(&self, point: &Point3) -> bool {
        let rotaries_ok = true; // rotary axes have no limits in this model
        point.x >= self.x_min
            && point.x <= self.x_max
            && point.y >= self.y_min
            && point.y <= self.y_max
            && point.z >= self.z_min
            && point.z <= self.z_max
            && rotaries_ok
    }
}

/// Standard "not supported in this phase" error for the region variant the
/// engine cannot resolve yet.
pub(crate) fn region_between_marks_unsupported(start: &str, end: &str) -> RosettaError {
    RosettaError::Unsupported {
        what: format!("region between marks \"{start}\"..\"{end}\""),
        phase: "2 (marker standard undecided, TRF §15)".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_names_match_capability_resolution_strings() {
        // The built-in rules store ApplyTransform("Rotate") and
        // ApplyTransform("CutterCompBake") — these must be real ids.
        assert_eq!(
            TransformKind::Rotate { angle_deg: 90.0, center: Point2::new(0.0, 0.0) }.name(),
            "Rotate"
        );
        assert_eq!(
            TransformKind::CutterCompBake {
                side: CompSide::Left,
                tool_radius: 3.0,
                lead: None
            }
            .name(),
            "CutterCompBake"
        );
    }

    #[test]
    fn families_and_defaults_partition_the_enum() {
        let rotate = TransformKind::Translate { delta: Point3::new(1.0, 2.0, 3.0) };
        assert_eq!(rotate.family(), TransformFamily::Affine);
        assert!(rotate.default_requires_confirm());
        let override_ = TransformKind::FeedSpeedOverride { feed_factor: 0.8, speed_factor: 1.0 };
        assert_eq!(override_.family(), TransformFamily::Structural);
        assert!(!override_.default_requires_confirm());
        assert!(override_.default_reversible());
    }

    #[test]
    fn cycle_code_round_maps_known_cycles_only() {
        assert_eq!(CycleCode::from_code(&Code::G(83)), Some(CycleCode::G83));
        assert_eq!(CycleCode::G82.code(), Code::G(82));
        assert_eq!(CycleCode::from_code(&Code::G(1)), None);
    }

    #[test]
    fn artifact_trace_keeps_original_line_numbers() {
        // TRF-M-06 sanity: trace entries carry source lines, not output
        // positions — the emitter reorders, the trace must not.
        let entry = TraceEntry {
            step_index: 1,
            name: "CycleSubstitute".into(),
            note: None,
            affected_lines: vec![42],
        };
        let text = serde_yaml_ng::to_string(&entry).unwrap();
        let back: TraceEntry = serde_yaml_ng::from_str(&text).unwrap();
        assert_eq!(back, entry);
    }
}
