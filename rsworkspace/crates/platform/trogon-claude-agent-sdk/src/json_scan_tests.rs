#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::{Token, is_json_number, overflows, scan};

#[test]
fn every_number_and_null_outside_strings_is_a_token_in_text_order() {
    let scanned = scan(r#"{"1":[1, -2.5e3, null, "null 4", true], "n\"ull": null}"#);
    assert_eq!(
        scanned.tokens,
        vec![Token::Number("1"), Token::Number("-2.5e3"), Token::Null, Token::Null]
    );
    assert!(!scanned.respelled);
}

#[test]
fn a_number_past_the_double_range_is_spelled_zero() {
    let scanned = scan("[1e400,-1E309,1e308]");
    assert_eq!(scanned.text, "[0,0,1e308]");
    assert_eq!(
        scanned.tokens,
        vec![Token::Number("1e400"), Token::Number("-1E309"), Token::Number("1e308")]
    );
    assert!(scanned.respelled);
}

#[test]
fn only_numbers_in_the_json_grammar_overflow() {
    assert_eq!(overflows("1e400"), Some(false));
    assert_eq!(overflows("-1e400"), Some(true));
    assert_eq!(overflows("1e308"), None);
    for text in ["01e400", "1e400.5", "-1e400e1", "+1e400", "1.e400", ".1e400", "1e+-400"] {
        assert!(!is_json_number(text), "{text}");
        assert_eq!(overflows(text), None, "{text}");
    }
}
