//! Server-side internationalization for strings produced by Rust code.
//!
//! Dictionaries are the same JSON files the browser loads from `web/locales`.
//! They are embedded into the binary at compile time, so no files are read at runtime.
//! Lookup falls back to English when a key is missing in the requested language.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Language used when the request does not name a supported language.
pub(crate) const DEFAULT_LANGUAGE: &str = "en";

/// Languages with an embedded dictionary, in the order used for `Accept-Language` matching.
pub(crate) const SUPPORTED_LANGUAGES: [&str; 4] = ["en", "ru", "zh", "fa"];

/// Raw dictionary sources embedded at compile time.
const EN_JSON: &str = include_str!("../web/locales/en.json");
const RU_JSON: &str = include_str!("../web/locales/ru.json");
const ZH_JSON: &str = include_str!("../web/locales/zh.json");
const FA_JSON: &str = include_str!("../web/locales/fa.json");

/// Parsed dictionaries keyed by language code; built once on first use.
static DICTIONARIES: OnceLock<HashMap<&'static str, HashMap<String, String>>> = OnceLock::new();

/// Returns the parsed dictionaries, parsing the embedded JSON on the first call.
///
/// Invalid embedded JSON is a build-time defect, so parsing failure panics with a clear message.
fn dictionaries() -> &'static HashMap<&'static str, HashMap<String, String>> {
    DICTIONARIES.get_or_init(|| {
        let sources: [(&'static str, &str); 4] = [
            ("en", EN_JSON),
            ("ru", RU_JSON),
            ("zh", ZH_JSON),
            ("fa", FA_JSON),
        ];
        sources
            .into_iter()
            .map(|(language, source)| {
                let parsed: HashMap<String, String> = match serde_json::from_str::<Value>(source) {
                    Ok(Value::Object(map)) => map
                        .into_iter()
                        .filter_map(|(key, value)| value.as_str().map(|text| (key, text.to_string())))
                        .collect(),
                    _ => panic!("embedded locale {language} is not a JSON object"),
                };
                (language, parsed)
            })
            .collect()
    })
}

/// Returns the raw embedded JSON source of the dictionary for `language`.
///
/// Used by the Tauri desktop shell: the webview may run on a custom protocol
/// where fetching `web/locales/*.json` asset files is unreliable, so the
/// frontend asks a Tauri command for the dictionary text instead. The sources
/// are compile-time `include_str!` constants, so no filesystem access happens.
///
/// Returns `None` for languages outside [`SUPPORTED_LANGUAGES`].
///
/// Only referenced from the Tauri shell (`tauri_app.rs`), which is compiled
/// out of the `cnc-web` binary, so the axum-only build sees it as dead code.
#[allow(dead_code)]
pub(crate) fn raw_dictionary(language: &str) -> Option<&'static str> {
    match normalize(language)? {
        "en" => Some(EN_JSON),
        "ru" => Some(RU_JSON),
        "zh" => Some(ZH_JSON),
        "fa" => Some(FA_JSON),
        // `normalize` only returns codes from SUPPORTED_LANGUAGES, so this
        // arm exists purely to keep the match exhaustive.
        _ => None,
    }
}

/// Normalizes a language tag such as `ru-RU` or `zh_CN` to a supported base code.
///
/// Returns `None` when the base language is not in [`SUPPORTED_LANGUAGES`].
fn normalize(tag: &str) -> Option<&'static str> {
    let base = tag
        .trim()
        .split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    SUPPORTED_LANGUAGES
        .iter()
        .copied()
        .find(|language| *language == base)
}

/// Chooses the response language from an explicit `lang` query value, then from `Accept-Language`.
///
/// An explicit `lang` value always wins when it is supported; otherwise the first supported
/// entry of `Accept-Language` is used; otherwise [`DEFAULT_LANGUAGE`] is returned.
pub(crate) fn negotiate(lang_param: Option<&str>, accept_language: Option<&str>) -> &'static str {
    if let Some(language) = lang_param.and_then(normalize) {
        return language;
    }

    accept_language
        .unwrap_or("")
        .split(',')
        .filter_map(|entry| entry.split(';').next())
        .find_map(normalize)
        .unwrap_or(DEFAULT_LANGUAGE)
}

/// Translates `key` into `language`, substituting `{name}` placeholders from `params`.
///
/// Missing keys in the requested language fall back to English; a key missing everywhere is
/// returned unchanged so the omission is visible instead of silently empty.
pub(crate) fn translate(language: &str, key: &str, params: &[(&str, String)]) -> String {
    let all = dictionaries();
    let template = all
        .get(language)
        .and_then(|dictionary| dictionary.get(key))
        .or_else(|| all.get(DEFAULT_LANGUAGE).and_then(|dictionary| dictionary.get(key)))
        .map(String::as_str)
        .unwrap_or(key);

    substitute(template, params)
}

/// Replaces every `{name}` in `template` with the matching value from `params`.
///
/// Placeholders without a matching parameter are left unchanged.
fn substitute(template: &str, params: &[(&str, String)]) -> String {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(start) = rest.find('{') {
        output.push_str(&rest[..start]);
        let after = &rest[start..];
        match after.find('}') {
            Some(end) => {
                let name = &after[1..end];
                match params.iter().find(|(key, _)| *key == name) {
                    Some((_, value)) => output.push_str(value),
                    None => output.push_str(&after[..=end]),
                }
                rest = &after[end + 1..];
            }
            None => {
                output.push_str(after);
                rest = "";
            }
        }
    }
    output.push_str(rest);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that an explicit supported `lang` parameter takes precedence over headers.
    #[test]
    fn explicit_language_wins_over_accept_language() {
        assert_eq!(negotiate(Some("ru"), Some("zh-CN,en;q=0.5")), "ru");
    }

    /// Verifies that `Accept-Language` is honored when no `lang` parameter is given.
    #[test]
    fn accept_language_is_used_without_lang_parameter() {
        assert_eq!(negotiate(None, Some("fa-IR,fa;q=0.9,en;q=0.5")), "fa");
    }

    /// Verifies that unsupported values fall back to English.
    #[test]
    fn unsupported_languages_fall_back_to_english() {
        assert_eq!(negotiate(Some("de"), Some("fr")), DEFAULT_LANGUAGE);
    }

    /// Verifies that placeholders are substituted and unknown ones are kept.
    #[test]
    fn substitutes_known_placeholders_only() {
        let out = substitute("x {a} y {b}", &[("a", "1".to_string())]);
        assert_eq!(out, "x 1 y {b}");
    }

    /// Verifies that a key missing in a language falls back to the English text.
    #[test]
    fn missing_key_falls_back_to_english() {
        let text = translate("ru", "common.close", &[]);
        assert!(!text.is_empty());
        assert_ne!(text, "common.close");
    }

    /// Verifies that every supported language exposes a parseable raw source.
    #[test]
    fn raw_dictionaries_are_available_for_supported_languages() {
        for language in SUPPORTED_LANGUAGES {
            let raw = raw_dictionary(language).expect("supported language has a raw dictionary");
            assert!(serde_json::from_str::<Value>(raw).is_ok());
        }
        assert!(raw_dictionary("de").is_none());
    }
}
