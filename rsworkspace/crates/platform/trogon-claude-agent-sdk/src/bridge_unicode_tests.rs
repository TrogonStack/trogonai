use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

use super::*;

const UNICODE_GOLDENS: &str = include_str!("../tests/fixtures/bridge_create_session/unicode.json");

fn goldens() -> Vec<Value> {
    serde_json::from_str(UNICODE_GOLDENS).unwrap_or_default()
}

fn text(case: &Value, key: &str) -> String {
    case[key].as_str().unwrap_or_default().to_owned()
}

#[test]
fn lowercases_like_the_javascript_runtime() {
    let cases: Vec<Value> = goldens().into_iter().filter(|case| case["kind"] == "lower").collect();
    assert!(cases.len() > 20);
    for case in &cases {
        let input = text(case, "text");
        assert_eq!(js_lowercase(&input), text(case, "lower"), "{input:?}");
    }
}

#[test]
fn lowercases_every_non_ascii_character_like_the_javascript_runtime() {
    assert_eq!(js_lowercase("\u{130}"), "i\u{307}");
    assert_eq!(js_lowercase("\u{212a}\u{1e9e}\u{1c5}"), "k\u{df}\u{1c6}");
    assert_eq!(js_lowercase("GitHub.COM"), "github.com");
    assert_eq!(js_lowercase("A\u{3a3}"), "a\u{3c2}");
    assert_eq!(js_lowercase("A\u{3a3}B"), "a\u{3c3}b");
    assert_eq!(js_lowercase("\u{3a3}"), "\u{3c3}");
}

#[test]
fn normalizes_like_the_javascript_runtime() {
    let cases: Vec<Value> = goldens().into_iter().filter(|case| case["kind"] == "nfc").collect();
    assert!(cases.len() > 1000);
    for case in &cases {
        let input = text(case, "text");
        assert_eq!(input.nfc().collect::<String>(), text(case, "nfc"), "{input:?}");
    }
}

#[test]
fn classifies_characters_like_the_javascript_runtime() {
    assert!(is_space('\u{feff}') && is_space('\u{2029}') && is_space(' '));
    assert!(!is_space('\u{200b}') && !is_space('a'));
    assert!(is_forbidden('\u{0}') && is_forbidden('\u{ad}') && is_forbidden('\u{2800}'));
    assert!(is_forbidden('\u{e000}') && is_forbidden('\u{378}') && is_forbidden('\t'));
    assert!(!is_forbidden(' ') && !is_forbidden('a') && !is_forbidden('\u{301}'));
    assert!(is_mark('\u{301}') && !is_mark('a'));
    assert!(is_punctuation('.') && is_punctuation('\u{2013}') && !is_punctuation('a'));
}
