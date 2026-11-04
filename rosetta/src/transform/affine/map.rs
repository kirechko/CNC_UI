//! Matrix representation and coordinate-mapping operations for affine transforms.

use crate::error::{RosettaError, RosettaResult};
use crate::ir::toolpath::{Point2, Point3, Segment, SegmentKind};
use crate::transform::model::{MirrorAxes, TransformKind};
use crate::util::almost_equal;

/// A 2D similarity/affine map plus the scalar Z rule, shared by all kinds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AffineMap {
    /// Linear part rows: `x' = a*x + b*y + e`, `y' = c*x + d*y + f`.
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
    /// Z scales by `z_factor` then shifts by `z_delta`.
    pub z_factor: f64,
    pub z_delta: f64,
}

impl AffineMap {
    /// The identity map.
    pub fn identity() -> Self {
        AffineMap {
            a: 1.0,
            b: 0.0,
            c: 0.0,
            d: 1.0,
            e: 0.0,
            f: 0.0,
            z_factor: 1.0,
            z_delta: 0.0,
        }
    }

    /// Pure translation.
    pub fn translation(dx: f64, dy: f64, dz: f64) -> Self {
        AffineMap {
            e: dx,
            f: dy,
            z_delta: dz,
            ..AffineMap::identity()
        }
    }

    /// Uniform scale around the origin.
    pub fn uniform_scale(s: f64) -> Self {
        AffineMap {
            a: s,
            d: s,
            z_factor: s,
            ..AffineMap::identity()
        }
    }

    /// Determinant of the XY linear part; negative ⇒ reflection.
    pub fn det(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// Whether the XY part is a similarity (rotation+reflection+uniform
    /// scale): maps circles to circles. The tolerance is relative because
    /// `UnitsConvert` factors like 1/25.4 are small.
    pub fn is_similarity(&self, eps: f64) -> bool {
        let scale_sq = (self.a * self.a + self.c * self.c).sqrt();
        almost_equal(self.a, self.d, eps * scale_sq.max(1.0))
            && almost_equal(self.b, -self.c, eps * scale_sq.max(1.0))
            || almost_equal(self.a, -self.d, eps * scale_sq.max(1.0))
                && almost_equal(self.b, self.c, eps * scale_sq.max(1.0))
    }

    /// The uniform XY scale factor of a similarity (`None` otherwise).
    pub fn uniform_factor(&self) -> Option<f64> {
        self.is_similarity(1e-9)
            .then(|| (self.a * self.a + self.c * self.c).sqrt())
    }

    /// Map a planar point.
    pub fn map_xy(&self, p: Point2) -> Point2 {
        Point2::new(
            self.a * p.x + self.b * p.y + self.e,
            self.c * p.x + self.d * p.y + self.f,
        )
    }

    /// Map a space point including the scalar Z rule.
    pub fn map_point(&self, p: Point3) -> Point3 {
        let xy = self.map_xy(p.xy());
        Point3 {
            x: xy.x,
            y: xy.y,
            z: p.z * self.z_factor + self.z_delta,
            ..p
        }
    }

    /// Map a *vector* (no translation column) — arc offsets `I/J`.
    pub fn map_vector(&self, p: Point2) -> Point2 {
        Point2::new(self.a * p.x + self.b * p.y, self.c * p.x + self.d * p.y)
    }

    /// Build the map for an affine [`TransformKind`].
    pub fn from_kind(kind: &TransformKind) -> RosettaResult<Self> {
        Ok(match kind {
            TransformKind::Rotate { angle_deg, center } => {
                // Standard rotation about `center`: translate origin to (0,0),
                // rotate CCW for positive angle, translate back.
                let rad = angle_deg.to_radians();
                let (cos, sin) = (rad.cos(), rad.sin());
                AffineMap {
                    a: cos,
                    b: -sin,
                    c: sin,
                    d: cos,
                    e: center.x - cos * center.x + sin * center.y,
                    f: center.y - sin * center.x - cos * center.y,
                    z_factor: 1.0,
                    z_delta: 0.0,
                }
            }
            TransformKind::Translate { delta } => AffineMap::translation(delta.x, delta.y, delta.z),
            TransformKind::Scale { factors, origin } => {
                // x' = ox + sx*(x-ox) ⇒ a = sx, e = ox*(1-sx); same per axis.
                AffineMap {
                    a: factors.x,
                    b: 0.0,
                    c: 0.0,
                    d: factors.y,
                    e: origin.x * (1.0 - factors.x),
                    f: origin.y * (1.0 - factors.y),
                    z_factor: factors.z,
                    z_delta: origin.z * (1.0 - factors.z),
                }
            }
            TransformKind::Mirror { axes } => match axes {
                MirrorAxes::X => AffineMap {
                    a: -1.0,
                    ..AffineMap::identity()
                },
                MirrorAxes::Y => AffineMap {
                    d: -1.0,
                    ..AffineMap::identity()
                },
                // Mirroring both axes = 180° rotation, determinant +1: the
                // arc direction is *preserved*, a classic gotcha covered by
                // the det<0 rule below.
                MirrorAxes::Xy => AffineMap {
                    a: -1.0,
                    d: -1.0,
                    ..AffineMap::identity()
                },
            },
            TransformKind::UnitsConvert { from, to } => {
                AffineMap::uniform_scale(from.factor_to(*to))
            }
            other => {
                return Err(RosettaError::Validation {
                    reason: format!("{} is not an affine transform", other.name()),
                })
            }
        })
    }

    /// Rewrite one segment; `Err` for arc under non-similarity maps.
    pub fn map_segment(&self, segment: &Segment) -> RosettaResult<(Segment, bool)> {
        let mut out = segment.clone();
        out.from = self.map_point(segment.from);
        out.to = self.map_point(segment.to);
        let mut flipped = false;
        if segment.kind.is_arc() {
            let scale = self
                .uniform_factor()
                .ok_or_else(|| RosettaError::Geometry {
                    reason: format!(
                        "segment {}: non-uniform affine map would turn the arc into an ellipse",
                        segment.id
                    ),
                })?;
            let center = segment.center.ok_or_else(|| RosettaError::Geometry {
                reason: format!("segment {}: arc without center", segment.id),
            })?;
            out.center = Some(self.map_xy(center));
            if self.det() < 0.0 {
                out.kind = match segment.kind {
                    SegmentKind::ArcCw => SegmentKind::ArcCcw,
                    SegmentKind::ArcCcw => SegmentKind::ArcCw,
                    other => other,
                };
                flipped = true;
            }
            let _ = scale; // radius change is implicit in the mapped geometry
        }
        // Feed rates rescale under a pure uniform scale similarity
        // (`applies_to_feed`): that is what `UnitsConvert` needs, and it is
        // consistent for an isotropic `Scale`. Dwells are exempt — their
        // `feed` field stores milliseconds, a duration, not a distance rate.
        if self.applies_to_feed() && segment.kind != SegmentKind::Dwell {
            if let Some(feed) = segment.feed {
                out.feed = Some(feed * self.a);
            }
        }
        Ok((out, flipped))
    }

    /// Whether this map is a *pure uniform scale* (`a == d == z_factor`, no
    /// rotation, reflection, shear or translation) whose factor differs from
    /// 1 — the only case in which programmed feed rates must be rescaled
    /// (TRF §4А note; dwells are still exempt inside `map_segment`).
    pub fn applies_to_feed(&self) -> bool {
        self.a == self.d
            && self.z_factor == self.a
            && self.a != 1.0
            && self.b == 0.0
            && self.c == 0.0
            && self.e == 0.0
            && self.f == 0.0
            && self.z_delta == 0.0
    }
}
