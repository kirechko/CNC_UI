//! Lexical helpers for the G-code parser: comment stripping, `letter number`
//! word scanning, signed/exponential number reading, and tiny point utilities.

use crate::Position;

/// Removes `( ... )` comments and everything after `;`, as the JS parser does.
pub(super) fn strip_comments(line: &str) -> String {
    let mut without_parens = String::with_capacity(line.len());
    let mut depth_open = false;
    for ch in line.chars() {
        match ch {
            '(' => depth_open = true,
            ')' if depth_open => depth_open = false,
            _ if !depth_open => without_parens.push(ch),
            _ => {}
        }
    }
    without_parens
        .split(';')
        .next()
        .unwrap_or("")
        .to_string()
}

/// Reads `letter number` words from a stripped line, uppercasing the letter.
///
/// Numbers accept an optional sign, decimal point and exponent, mirroring
/// the JS regular expression used by the browser parser.
pub(super) fn read_words(line: &str) -> Vec<(char, f64)> {
    let chars: Vec<char> = line.chars().collect();
    let mut words = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let letter = chars[i];
        if !letter.is_ascii_alphabetic() {
            i += 1;
            continue;
        }
        let mut j = i + 1;
        while j < chars.len() && chars[j].is_whitespace() {
            j += 1;
        }
        if let Some((value, end)) = read_number(&chars, j) {
            words.push((letter.to_ascii_uppercase(), value));
            i = end;
        } else {
            i += 1;
        }
    }
    words
}

/// Reads a signed decimal number starting at `start`, returning its value and end index.
fn read_number(chars: &[char], start: usize) -> Option<(f64, usize)> {
    let mut end = start;
    if end < chars.len() && (chars[end] == '+' || chars[end] == '-') {
        end += 1;
    }
    let digits_start = end;
    while end < chars.len() && chars[end].is_ascii_digit() {
        end += 1;
    }
    let mut has_digits = end > digits_start;
    if end < chars.len() && chars[end] == '.' {
        end += 1;
        let frac_start = end;
        while end < chars.len() && chars[end].is_ascii_digit() {
            end += 1;
        }
        has_digits |= end > frac_start;
    }
    if !has_digits {
        return None;
    }
    if end < chars.len() && (chars[end] == 'e' || chars[end] == 'E') {
        let mut exp = end + 1;
        if exp < chars.len() && (chars[exp] == '+' || chars[exp] == '-') {
            exp += 1;
        }
        let exp_digits_start = exp;
        while exp < chars.len() && chars[exp].is_ascii_digit() {
            exp += 1;
        }
        if exp > exp_digits_start {
            end = exp;
        }
    }
    let text: String = chars[start..end].iter().collect();
    text.parse::<f64>().ok().map(|value| (value, end))
}

/// Euclidean distance between two 3D points, in millimetres.
pub(super) fn distance_3d(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let dz = b[2] - a[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Converts an internal `[x, y, z]` point to a [`Position`] with unused rotary axes zeroed.
pub(super) fn to_position(point: [f64; 3]) -> Position {
    Position {
        x: point[0],
        y: point[1],
        z: point[2],
        a: 0.0,
        b: 0.0,
        c: 0.0,
    }
}
