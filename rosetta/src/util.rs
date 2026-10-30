//! Small shared helpers used across all Rosetta subsystems.
//!
//! Keeping this module free of any IR knowledge means both `rosetta::io` and
//! `rosetta::transform` can reuse the same number formatting rules, which is
//! required for deterministic emission (ТЗ №1 principle 3: "deterministic
//! translation" — the same IR must always produce byte-identical output).

/// Formats a coordinate/feed/parameter value the way G-code emitters expect:
/// up to 6 decimal places with redundant trailing zeros stripped, never using
/// scientific notation for values in the practical CNC range.
///
/// * `12.5`   -> `"12.5"`
/// * `12.0`   -> `"12"`
/// * `-0.0000004` -> `"0"` (below emission precision; avoids "-0" output)
///
/// Non-finite inputs are stringified as-is so callers can still detect them
/// (capability/limit validators treat them as errors before emission).
pub fn fmt_num(value: f64) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    // `{:.6}` rounds to micron precision, which is finer than any real
    // machine resolution we target, and never switches to exponent form.
    let text = format!("{value:.6}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    // Normalize the negative-zero artifact of rounding (e.g. -0.0000001).
    if trimmed == "-0" {
        return "0".to_owned();
    }
    trimmed.to_owned()
}

/// Epsilon comparison used by arc/segment continuity checks in the transform
/// validator (`TRF` §11.3 "path integrity"). `eps` is expected in machine
/// units (millions of a millimeter is plenty for parse/emit roundtrips).
pub fn almost_equal(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_with_trailing_zeros_stripped() {
        assert_eq!(fmt_num(12.5), "12.5");
        assert_eq!(fmt_num(12.0), "12");
        assert_eq!(fmt_num(-0.125), "-0.125");
        assert_eq!(fmt_num(0.0000004), "0");
        assert_eq!(fmt_num(-0.0000004), "0");
    }

    #[test]
    fn non_finite_values_pass_through() {
        assert_eq!(fmt_num(f64::NAN), "NaN");
        assert!(fmt_num(f64::INFINITY).contains("inf"));
    }

    #[test]
    fn almost_equal_respects_epsilon() {
        assert!(almost_equal(1.0, 1.0 + 1e-9, 1e-6));
        assert!(!almost_equal(1.0, 1.1, 1e-6));
    }
}
