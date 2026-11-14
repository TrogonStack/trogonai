use super::*;

#[test]
fn trims_the_javascript_white_space_set() {
    assert_eq!(js_trim("\u{feff}\u{a0} a b\u{2028}\n"), "a b");
    assert!(is_js_space('\u{3000}'));
    assert!(!is_js_space('\u{200b}'));
}

#[test]
fn reads_numbers_like_the_number_function() {
    assert_eq!(js_number(""), 0.0);
    assert_eq!(js_number("  12 "), 12.0);
    assert_eq!(js_number("1e3"), 1000.0);
    assert_eq!(js_number(".5"), 0.5);
    assert_eq!(js_number("0x1f"), 31.0);
    assert_eq!(js_number("0O17"), 15.0);
    assert_eq!(js_number("0b11"), 3.0);
    assert_eq!(js_number("Infinity"), f64::INFINITY);
    assert_eq!(js_number("+Infinity"), f64::INFINITY);
    assert_eq!(js_number("-Infinity"), f64::NEG_INFINITY);
    for text in ["0x", "0xg", "abc", "1_0", "--1", "infinity", "1e"] {
        assert!(js_number(text).is_nan(), "{text}");
    }
}

#[test]
fn accepts_only_safe_integers() {
    assert_eq!(safe_integer(7.0), Some(7));
    assert_eq!(safe_integer(-0.0), Some(0));
    assert_eq!(safe_integer(9_007_199_254_740_991.0), Some(9_007_199_254_740_991));
    assert_eq!(safe_integer(9_007_199_254_740_992.0), None);
    assert_eq!(safe_integer(1.5), None);
    assert_eq!(safe_integer(f64::NAN), None);
    assert_eq!(safe_integer(f64::INFINITY), None);
}

#[test]
fn keeps_plain_one_line_messages() {
    assert_eq!(
        plain_detail("Grouping not found"),
        Some("Grouping not found".to_owned())
    );
    assert_eq!(plain_detail("use `x` here"), Some("use `x` here".to_owned()));
    assert_eq!(plain_detail("x\u{301} stays"), Some("x\u{301} stays".to_owned()));
    assert!(plain_detail(&"a".repeat(1024)).is_some());
}

#[test]
fn drops_messages_the_cleaning_would_change() {
    assert_eq!(plain_detail(&"a".repeat(1025)), None);
    assert_eq!(plain_detail("a  b"), None);
    assert_eq!(plain_detail(" a"), None);
    assert_eq!(plain_detail("a\tb"), None);
    assert_eq!(plain_detail("e\u{301}"), None);
    assert_eq!(plain_detail("a\u{ff40}b"), None);
    assert_eq!(plain_detail(".\u{301}\u{302}a"), None);
    assert_eq!(plain_detail("\u{301}a"), None);
}

#[test]
fn drops_messages_with_hidden_or_unassigned_characters() {
    for text in [
        "a\u{0}b",
        "a\u{200b}b",
        "a\u{e000}b",
        "a\u{2800}b",
        "a\u{ad}b",
        "a\u{34f}b",
        "a\u{61c}b",
        "a\u{115f}b",
        "a\u{17b4}b",
        "a\u{180b}b",
        "a\u{202a}b",
        "a\u{2060}b",
        "a\u{3164}b",
        "a\u{fe00}b",
        "a\u{ffa0}b",
        "a\u{fff0}b",
        "a\u{1bca0}b",
        "a\u{1d173}b",
        "a\u{e0001}b",
        "a\u{e0100}b",
        "a\u{378}b",
        "a\u{2028}b",
    ] {
        assert_eq!(plain_detail(text), None, "{text:?}");
    }
}
