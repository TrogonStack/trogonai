use super::*;

#[test]
fn whitespace_matches_the_ecmascript_class() {
    for ch in [
        '\t', '\n', '\u{b}', '\u{c}', '\r', ' ', '\u{a0}', '\u{1680}', '\u{2003}', '\u{2028}', '\u{2029}', '\u{202f}',
        '\u{205f}', '\u{3000}', '\u{feff}',
    ] {
        assert!(is_js_whitespace(ch), "{ch:?}");
    }
    for ch in ['a', '0', '\u{85}', '\u{200b}', '\u{180e}'] {
        assert!(!is_js_whitespace(ch), "{ch:?}");
    }
}
