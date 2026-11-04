//! Affine transforms (ТЗ №3 §4А, Phase A): `Rotate`, `Translate`, `Scale`,
//! `Mirror`, `UnitsConvert`.
//!
//! All five reduce to one 2D affine matrix for the XY plane plus a scalar
//! rule for Z, which is exactly what makes them *affine* — no new geometry,
//! only recomputed coordinates:
//!
//! ```text
//! | x' |   | a  b | | x |   | e |
//! | y' | = | c  d | | y | + | f |
//! z' = z * z_factor + z_delta
//! ```
//!
//! * Rotation is the only kind with a nonzero `b`/`c` beyond scaling;
//!   `det = a*d − b*c`; `det < 0` means a reflection, which flips arc
//!   direction (a mirrored clockwise circle is counter-clockwise).
//! * Arcs survive an affine map only when it is a *similarity* (uniform
//!   scale + rotation + reflection + translation); a non-uniform `Scale`
//!   turns circles into ellipses, and the engine refuses with a
//!   [`RosettaError::Geometry`] instead of emitting fake arcs (principle 4).
//! * Cycle frames (`G81`…) have no segments, so their `X/Y/Z` address words
//!   are rewritten directly by the same matrix; arc offsets `I/J` are
//!   *vectors* and use the linear part without the translation column.

mod executor;
mod map;

#[cfg(test)]
mod tests;

pub use executor::AffineTransform;
pub use map::AffineMap;
