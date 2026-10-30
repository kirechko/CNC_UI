//! Toolpath IR — the canonical representation of a parsed G-code program.
//!
//! Implements `CORE-T-01`: Rosetta Transform (`TRF-C-01`) uses these
//! structures as its single source of truth, and every mutation must go
//! through them so the result stays traceable (`CORE-T-06` / `TRF-C-06`).
//!
//! # Shape of the model
//!
//! A program is modeled as two synchronized views:
//!
//! * [`Toolpath::codes`] — the ordered stream of [`CodeFrame`]s, i.e. the
//!   *command* view of each source line (modal G-codes, cycles, comments).
//!   Detection and structural transforms work on this view.
//! * [`Toolpath::segments`] — the *geometry* view: motion primitives
//!   (linear/arc/dwell) with start/end points. Affine transforms work on
//!   this view and then re-render themselves back into the frame stream via
//!   the emitter (`rosetta::io::gcode`), which interleaves both views by
//!   their `line` numbers.
//!
//! Keeping both views in one struct (rather than deriving geometry on the
//! fly) is what makes round-trips `parse → transform → emit → parse` stable
//! and testable (ТЗ №3 §12.2).

use serde::{Deserialize, Serialize};

/// Units of the source program (`G20`/`G21`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Units {
    Millimeters,
    Inches,
}

impl Default for Units {
    fn default() -> Self {
        Units::Millimeters
    }
}

impl Units {
    /// Scale factor converting one unit of `self` into `other`; 1.0 when they
    /// match, 25.4 mm/in and its reciprocal otherwise (`UnitsConvert`).
    pub fn factor_to(self, other: Units) -> f64 {
        match (self, other) {
            (Units::Millimeters, Units::Inches) => 1.0 / 25.4,
            (Units::Inches, Units::Millimeters) => 25.4,
            _ => 1.0,
        }
    }
}

/// A 2D point in toolpath space.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point2 {
    pub x: f64,
    pub y: f64,
}

impl Point2 {
    /// Construct a point from coordinates.
    pub fn new(x: f64, y: f64) -> Self {
        Point2 { x, y }
    }
}

/// A 3D point plus the rotary/auxiliary axes we carry through transforms.
/// `a`/`b`/`c` are rotary axes; `None` means "no explicit address word".
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Point3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c: Option<f64>,
}

impl Point3 {
    /// Construct a point with only the linear axes addressed.
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Point3 { x, y, z, a: None, b: None, c: None }
    }

    /// The planar projection used by affine XY transforms.
    pub fn xy(&self) -> Point2 {
        Point2::new(self.x, self.y)
    }
}

/// Identifier of a [`Segment`] inside one [`Toolpath`].
pub type SegmentId = u32;

/// Kind of a motion primitive (`Segment.kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SegmentKind {
    /// G0 — non-cutting move.
    Rapid,
    /// G1 — feeding move.
    Linear,
    /// G2 — clockwise arc in the XY plane.
    ArcCw,
    /// G3 — counterclockwise arc in the XY plane.
    ArcCcw,
    /// G4 — time-based dwell (duration kept in `feed` as milliseconds).
    Dwell,
}

impl SegmentKind {
    /// True for the two arc kinds — transforms must treat those specially
    /// (direction flips under mirroring, radius scaling).
    pub fn is_arc(self) -> bool {
        matches!(self, SegmentKind::ArcCw | SegmentKind::ArcCcw)
    }
}

/// A geometric primitive of the path (`CORE-T-01`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub id: SegmentId,
    pub kind: SegmentKind,
    /// Start point; the emitter relies on `from == previous.to` for
    /// continuity validation (ТЗ №3 §11.3).
    pub from: Point3,
    pub to: Point3,
    /// Arc center in XY; `Some` exactly for arc kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub center: Option<Point2>,
    /// Feed rate (mm/min or in/min) for motion kinds; dwell milliseconds for
    /// [`SegmentKind::Dwell`]; `None` when modal state supplies the value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feed: Option<f64>,
    /// 1-based source line this segment came from (line tracing, `TRF-C-06`).
    pub line: usize,
}

/// A machine code as referenced by the capability matrix and resolution
/// registry. G and M codes are numeric; everything else (unknown words) is
/// preserved verbatim for pass-through.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Code {
    G(u16),
    M(u16),
    Other(String),
}

impl Code {
    /// Parse a bare code token such as `"G81"` or `"M6"` (case-insensitive).
    pub fn parse(token: &str) -> Option<Self> {
        let upper = token.trim().to_ascii_uppercase();
        if let Some(rest) = upper.strip_prefix('G') {
            rest.parse::<u16>().ok().map(Code::G)
        } else if let Some(rest) = upper.strip_prefix('M') {
            rest.parse::<u16>().ok().map(Code::M)
        } else {
            None
        }
    }

    /// Canonical display form (`"G0"`, not `"G00"`).
    pub fn canonical(&self) -> String {
        match self {
            Code::G(n) => format!("G{n}"),
            Code::M(n) => format!("M{n}"),
            Code::Other(text) => text.clone(),
        }
    }
}

impl std::fmt::Display for Code {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.canonical())
    }
}

/// One source line rendered as a command frame (`CORE-T-01` / detection).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CodeFrame {
    /// 1-based source line number, preserved for tracing and reports.
    pub line: usize,
    /// All G/M codes appearing on the line, in source order.
    pub codes: Vec<Code>,
    /// Address words on the line other than G/M (e.g. `X10`, `F300`).
    /// Kept verbatim so unknown lines survive a round-trip unchanged.
    #[serde(default)]
    pub words: Vec<(char, f64)>,
    /// Original text of the line (without comments) for faithful emission.
    #[serde(default)]
    pub raw: String,
    /// Comment text (`; …` / `( … )`) if the line carried one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    /// Set by `CycleSubstitute` (TRF-M-06): the macro-emulated replacement
    /// lines that must be emitted *in place of* the original codes, while
    /// the frame still preserves the original line for tracing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub substitute: Option<Vec<String>>,
}

impl CodeFrame {
    /// True when the frame mentions `code` anywhere on its line.
    pub fn mentions(&self, code: &Code) -> bool {
        self.codes.iter().any(|c| c == code)
    }
}

/// The Toolpath IR root (`CORE-T-01`).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Toolpath {
    /// Units active at end of parsing (G20/G21 are modal).
    #[serde(default)]
    pub units: Units,
    /// Geometry view: motion primitives in execution order.
    #[serde(default)]
    pub segments: Vec<Segment>,
    /// Command view: frames in source order (including non-motion lines).
    #[serde(default)]
    pub codes: Vec<CodeFrame>,
    /// Free-form header comments collected from the top of the program.
    #[serde(default)]
    pub header_comments: Vec<String>,
}

impl Toolpath {
    /// Axis-aligned bounding box over segment endpoints:
    /// `((min_x, min_y, min_z), (max_x, max_y, max_z))`.
    /// Returns `None` for an empty path — validators treat that as a warning,
    /// not an error, because comment-only files are legal.
    pub fn bounds(&self) -> Option<((f64, f64, f64), (f64, f64, f64))> {
        let mut min = (f64::INFINITY, f64::INFINITY, f64::INFINITY);
        let mut max = (f64::NEG_INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
        // Fully annotate the closure parameters: with `&mut _` inference the
        // tuple element types would be ambiguous at the call sites below.
        let extend = |p: &Point3, min: &mut (f64, f64, f64), max: &mut (f64, f64, f64)| {
            min.0 = min.0.min(p.x);
            min.1 = min.1.min(p.y);
            min.2 = min.2.min(p.z);
            max.0 = max.0.max(p.x);
            max.1 = max.1.max(p.y);
            max.2 = max.2.max(p.z);
        };
        for segment in &self.segments {
            extend(&segment.from, &mut min, &mut max);
            extend(&segment.to, &mut min, &mut max);
        }
        if self.segments.is_empty() {
            None
        } else {
            Some((min, max))
        }
    }

    /// Total count of geometry segments — used by artifact summaries.
    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_factor_is_exact() {
        assert!((Units::Inches.factor_to(Units::Millimeters) - 25.4).abs() < 1e-12);
        assert!((Units::Millimeters.factor_to(Units::Inches) * 25.4 - 1.0).abs() < 1e-12);
        assert_eq!(Units::Millimeters.factor_to(Units::Millimeters), 1.0);
    }

    #[test]
    fn code_parse_and_canonical_form() {
        assert_eq!(Code::parse("g81"), Some(Code::G(81)));
        assert_eq!(Code::parse("M6"), Some(Code::M(6)));
        assert_eq!(Code::G(0).canonical(), "G0");
        assert_eq!(Code::parse("L1"), None);
    }

    #[test]
    fn bounds_spans_all_endpoints() {
        let mut path = Toolpath::default();
        path.segments.push(Segment {
            id: 0,
            kind: SegmentKind::Linear,
            from: Point3::new(0.0, 0.0, 0.0),
            to: Point3::new(10.0, 5.0, -2.0),
            center: None,
            feed: Some(100.0),
            line: 1,
        });
        let ((min_x, min_y, min_z), (max_x, max_y, max_z)) = path.bounds().unwrap();
        assert_eq!((min_x, min_y, min_z), (0.0, 0.0, -2.0));
        assert_eq!((max_x, max_y, max_z), (10.0, 5.0, 0.0));
        assert!(Toolpath::default().bounds().is_none());
    }

    #[test]
    fn frame_mentions_code() {
        let frame = CodeFrame {
            line: 3,
            codes: vec![Code::G(91), Code::G(38), Code::M(0)],
            words: vec![('Z', -5.0), ('F', 100.0)],
            raw: "G91 G38.2 Z-5 F100".into(),
            comment: None,
            substitute: None,
        };
        assert!(frame.mentions(&Code::M(0)));
        assert!(!frame.mentions(&Code::G(1)));
    }
}
