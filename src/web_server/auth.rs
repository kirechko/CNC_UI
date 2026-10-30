//! Simple HTTP Basic authorization for the embedded web server (task: add
//! basic auth with a setting that disables it).
//!
//! The backend has no user database, sessions or roles — it guards every
//! route (API + static UI) with one username/password pair checked in
//! constant time against the `Authorization: Basic` header. Browsers render
//! the native login prompt, so no UI changes are required, and a user who
//! authenticated once is silent for all subsequent `fetch` calls because the
//! credentials are cached for the origin.
//!
//! Configuration (environment variables, read once at startup by
//! [`auth_settings`]):
//!
//! * `CNC_UI_AUTH` — `off`/`none`/`0` disables the check even when
//!   credentials are configured; `on`/`1` forces it on. Absent/other values
//!   mean "auto": enabled only when both `CNC_UI_AUTH_USER` and
//!   `CNC_UI_AUTH_PASSWORD` are present. Auto keeps the historical
//!   zero-config behavior (open server on a trusted LAN) while making auth a
//!   single environment variable away.
//! * `CNC_UI_AUTH_USER` / `CNC_UI_AUTH_PASSWORD` — the one accepted
//!   credential pair. With `CNC_UI_AUTH=on` and no password, a random one is
//!   generated and printed to the server log at startup.
//!
//! The desktop (Tauri) process never starts this module: its UI is not
//! exposed over TCP at all.

use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

/// Resolved authorization mode of the web server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AuthMode {
    /// Authorization disabled: every request passes through untouched.
    Disabled,
    /// One accepted Basic credential pair.
    Enabled { username: String, password: String },
}

/// Process-wide auth mode, initialized once by the server bootstrap.
///
/// A `OnceLock` (rather than middleware state) is used because the axum
/// router in `server.rs` combines routes with a `fallback_service`; a
/// `route_layer` over such a router fails trait resolution, while a plain
/// `from_fn` middleware reading this static works for every request shape.
static MODE: OnceLock<AuthMode> = OnceLock::new();

/// Publishes the resolved mode so [`require_auth`] can read it.
pub(crate) fn set_mode(mode: AuthMode) {
    let _ = MODE.set(mode);
}

/// Whether authorization is currently enforced. Useful for handlers that
/// want to report the security posture back to the UI.
#[allow(dead_code)]
pub(crate) fn is_enabled() -> bool {
    matches!(MODE.get(), Some(AuthMode::Enabled { .. }))
}

/// Username used when auth is forced on without `CNC_UI_AUTH_USER`.
pub(crate) const DEFAULT_AUTH_USER: &str = "operator";

/// `CNC_UI_AUTH` values that force the server open.
const OFF_VALUES: [&str; 3] = ["off", "none", "0"];
/// `CNC_UI_AUTH` values that force authorization on.
const ON_VALUES: [&str; 2] = ["on", "1"];

/// Base64 alphabet of the canonical (non-URL) standard encoding; used by the
/// test helper encoder only (the decoder maps characters directly).
#[cfg(test)]
const BASE64_ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Alphabet for generated passwords: alphanumeric only (no `+`/`/`), so the
/// value survives copy-paste through shells and config files unescaped. The
/// look-alike characters `0`/`O` and `1`/`l`/`I` are dropped for readability.
const PASSWORD_ALPHABET: &[u8] = b"abcdefghijkmnopqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Generates 24 random alphanumeric characters from the OS entropy source.
///
/// Used as a fallback password when auth is forced on without a configured
/// password; the operator reads it from the startup log. Falls back to a
/// time-derived filler only if `/dev/urandom` cannot be read (never on the
/// supported Linux targets).
fn random_password() -> String {
    use std::io::Read;

    let mut bytes = [0_u8; 24];
    let filled = std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_ok();
    if !filled {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.subsec_nanos())
            .unwrap_or(0x5EED_u32);
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = (nanos >> (index % 24)) as u8;
        }
    }
    bytes
        .iter()
        .map(|byte| PASSWORD_ALPHABET[(*byte as usize) % PASSWORD_ALPHABET.len()] as char)
        .collect()
}

/// Resolves the [`AuthMode`] from explicit inputs.
///
/// Split out from [`auth_settings`] so the decision logic is unit-testable
/// without mutating the process environment.
///
/// @param flag raw `CNC_UI_AUTH` value (None when unset).
/// @param user raw `CNC_UI_AUTH_USER` value.
/// @param password raw `CNC_UI_AUTH_PASSWORD` value.
/// @param generate password generator used only in forced-on mode.
/// @returns The mode plus a short human-readable reason for the startup log.
fn resolve_mode(
    flag: Option<&str>,
    user: Option<&str>,
    password: Option<&str>,
    generate: impl FnOnce() -> String,
) -> (AuthMode, String) {
    let flag = flag.unwrap_or_default().trim().to_ascii_lowercase();
    let user = user
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let password = password
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    // Explicit disable wins over everything else: that is the "setting that
    // allows turning authorization off" the task asks for.
    if OFF_VALUES.contains(&flag.as_str()) {
        return (AuthMode::Disabled, "CNC_UI_AUTH=off".to_string());
    }

    if ON_VALUES.contains(&flag.as_str()) {
        return (
            AuthMode::Enabled {
                username: user.unwrap_or_else(|| DEFAULT_AUTH_USER.to_string()),
                password: password.unwrap_or_else(generate),
            },
            "CNC_UI_AUTH=on".to_string(),
        );
    }

    // Auto mode: a complete credential pair enables protection; otherwise the
    // server keeps its historical open behavior.
    match (user, password) {
        (Some(username), Some(pass)) => (
            AuthMode::Enabled {
                username,
                password: pass,
            },
            "credentials configured".to_string(),
        ),
        _ => (AuthMode::Disabled, "no credentials configured".to_string()),
    }
}

/// Reads the environment once and resolves the [`AuthMode`] plus the reason
/// string used in the startup log line.
pub(crate) fn auth_settings() -> (AuthMode, String) {
    resolve_mode(
        std::env::var("CNC_UI_AUTH").ok().as_deref(),
        std::env::var("CNC_UI_AUTH_USER").ok().as_deref(),
        std::env::var("CNC_UI_AUTH_PASSWORD").ok().as_deref(),
        random_password,
    )
}

/// Minimal standard-alphabet base64 decoder (`A-Za-z0-9+/` + `=` padding).
///
/// Implemented locally to keep the dependency tree unchanged (the workspace
/// has no base64 crate). Only what Basic headers need: canonical padded
/// input; malformed sequences yield `None`.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    fn sextet(byte: u8) -> Option<u32> {
        match byte {
            b'A'..=b'Z' => Some(u32::from(byte - b'A')),
            b'a'..=b'z' => Some(u32::from(byte - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(byte - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let bytes = input.as_bytes();
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        return None;
    }
    // Padding may only appear in the last one or two positions of the final
    // group; anywhere else is a malformed token.
    if let Some(position) = bytes.iter().position(|byte| *byte == b'=') {
        if position < bytes.len() - 2 || bytes[position..].iter().any(|byte| *byte != b'=') {
            return None;
        }
    }

    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for group in bytes.chunks_exact(4) {
        // Count how many trailing padding characters the group carries: they
        // mark how many of the three output bytes are real.
        let pad = group.iter().rev().take_while(|byte| **byte == b'=').count();
        if pad == 4 {
            return None; // "====" is not a valid final group
        }
        let mut bits = 0_u32;
        for byte in group {
            bits <<= 6;
            if *byte != b'=' {
                bits |= sextet(*byte)?;
            }
        }
        let produced = 3 - pad;
        for index in 0..produced {
            // Most significant byte first: 24-bit little window above padding.
            out.push(((bits >> (16 - 8 * index)) & 0xFF) as u8);
        }
    }
    Some(out)
}

/// Encodes `text` as canonical base64 (test helper; also handy for debugging).
#[cfg(test)]
fn base64_encode(text: &str) -> String {
    let mut out = String::new();
    for chunk in text.as_bytes().chunks(3) {
        let mut bits = 0_u32;
        for byte in chunk {
            bits = (bits << 8) | u32::from(*byte);
        }
        bits <<= 8 * (3 - chunk.len());
        let groups = match chunk.len() {
            3 => vec![
                BASE64_ALPHABET[((bits >> 18) & 0x3F) as usize],
                BASE64_ALPHABET[((bits >> 12) & 0x3F) as usize],
                BASE64_ALPHABET[((bits >> 6) & 0x3F) as usize],
                BASE64_ALPHABET[(bits & 0x3F) as usize],
            ],
            2 => vec![
                BASE64_ALPHABET[((bits >> 18) & 0x3F) as usize],
                BASE64_ALPHABET[((bits >> 12) & 0x3F) as usize],
                BASE64_ALPHABET[((bits >> 6) & 0x3F) as usize],
                b'=',
            ],
            _ => vec![
                BASE64_ALPHABET[((bits >> 18) & 0x3F) as usize],
                BASE64_ALPHABET[((bits >> 12) & 0x3F) as usize],
                b'=',
                b'=',
            ],
        };
        for byte in groups {
            out.push(char::from(byte));
        }
    }
    out
}

/// Byte-slice equality that does not short-circuit on the first difference.
///
/// Credential comparison must not leak the matching prefix length through
/// timing; this is the tiny stand-in for a dedicated constant-time crate.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        // Equalize the work: fold the shorter slice against the longer one so
        // the early-exit path is not observable as a timing difference.
        let (longer, shorter) = if a.len() > b.len() { (a, b) } else { (b, a) };
        let mut diff = (longer.len() ^ shorter.len()) as u8;
        for (index, byte) in longer.iter().enumerate() {
            diff |= byte
                .wrapping_add(0)
                ^ shorter
                    .get(index % shorter.len().max(1))
                    .copied()
                    .unwrap_or(0);
        }
        // The lengths already differ, so the answer is false; the loop above
        // only equalizes the timing of that answer.
        let _ = diff;
        return false;
    }
    let mut diff = 0_u8;
    for (left, right) in a.iter().zip(b) {
        diff |= left ^ right;
    }
    diff == 0
}

/// Parses one `Authorization: Basic <base64>` header value into the
/// username/password pair, splitting at the FIRST colon per RFC 7617.
///
/// The scheme name is matched case-insensitively; anything else (Bearer,
/// bogus base64, non-UTF-8 payload, missing colon) yields `None`.
fn parse_basic(header: &str) -> Option<(String, String)> {
    let (scheme, token) = header.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Basic") {
        return None;
    }
    let decoded = base64_decode(token.trim())?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, password) = text.split_once(':')?;
    Some((user.to_string(), password.to_string()))
}

/// Axum middleware enforcing the configured [`AuthMode`].
///
/// The mode lives in the process-wide [`MODE`] static set by
/// [`set_mode`] at startup, so this middleware needs no state generic and
/// composes cleanly with the router's `fallback_service`. A missing or
/// invalid credential answers with `401` + `WWW-Authenticate`, which makes
/// browsers show the native login dialog.
pub(crate) async fn require_auth(request: axum::extract::Request, next: Next) -> Response {
    let (expected_user, expected_password) = match MODE.get() {
        None | Some(AuthMode::Disabled) => return next.run(request).await,
        Some(AuthMode::Enabled { username, password }) => (username.as_str(), password.as_str()),
    };

    let provided = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_basic);

    let accepted = match provided {
        Some((user, password)) => {
            constant_time_eq(user.as_bytes(), expected_user.as_bytes())
                && constant_time_eq(password.as_bytes(), expected_password.as_bytes())
        }
        None => false,
    };

    if accepted {
        return next.run(request).await;
    }

    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(
            header::WWW_AUTHENTICATE,
            "Basic realm=\"CNC UI\", charset=\"UTF-8\"",
        )
        // A text body helps non-browser clients (curl -v) see why they failed.
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Body::from(
            "401 Unauthorized: CNC UI requires Basic authentication",
        ))
        .expect("static 401 response is always valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_basic_pairs() {
        for text in ["a", "ab", "abc", "abcd", "operator:s3cret", "a:b:c"] {
            let encoded = base64_encode(text);
            assert_eq!(
                base64_decode(&encoded).and_then(|bytes| String::from_utf8(bytes).ok()),
                Some(text.to_string()),
                "round trip of {text:?} via {encoded}"
            );
        }
        assert!(base64_decode("not base64!").is_none());
        assert!(base64_decode("YWJjZA").is_none()); // missing padding
        assert!(base64_decode("=abc").is_none()); // misplaced padding
    }

    #[test]
    fn parse_basic_splits_at_first_colon() {
        let header_line = format!("Basic {}", base64_encode("a:b:c"));
        let (user, pass) = parse_basic(&header_line).expect("pair");
        assert_eq!(user, "a");
        assert_eq!(pass, "b:c");
        assert!(parse_basic("Bearer xyz").is_none());
    }

    #[test]
    fn constant_time_eq_matches_semantics() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"secreu"));
        assert!(!constant_time_eq(b"secret", b"sec"));
        assert!(!constant_time_eq(b"sec", b"secret"));
    }

    #[test]
    fn explicit_off_flag_disables_auth() {
        let (mode, _) = resolve_mode(
            Some("off"),
            Some("operator"),
            Some("secret"),
            || "generated".to_string(),
        );
        assert_eq!(mode, AuthMode::Disabled);
    }

    #[test]
    fn auto_mode_enables_only_with_a_full_credential_pair() {
        let (mode, reason) = resolve_mode(None, Some("operator"), Some("secret"), || {
            "generated".to_string()
        });
        assert_eq!(
            mode,
            AuthMode::Enabled {
                username: "operator".into(),
                password: "secret".into()
            }
        );
        assert!(reason.contains("credentials"));

        let (half, _) = resolve_mode(None, Some("operator"), None, || "generated".to_string());
        assert_eq!(half, AuthMode::Disabled);
    }

    #[test]
    fn forced_on_mode_generates_a_password_when_none_configured() {
        let (mode, _) = resolve_mode(Some("on"), None, None, || "generated".to_string());
        assert_eq!(
            mode,
            AuthMode::Enabled {
                username: DEFAULT_AUTH_USER.into(),
                password: "generated".into()
            }
        );
    }

    #[test]
    fn generated_passwords_are_distinct_and_alphanumeric() {
        let first = random_password();
        let second = random_password();
        assert_eq!(first.len(), 24);
        assert!(first.chars().all(|c| c.is_ascii_alphanumeric()));
        // Two OS-entropy draws must not collide in practice.
        assert_ne!(first, second);
    }
}
