use super::*;

#[test]
fn quote_wraps_a_plain_word() {
    assert_eq!(shell_quote("abc"), "'abc'");
}

#[test]
fn quote_closes_and_reopens_around_an_embedded_quote() {
    assert_eq!(shell_quote("a'b"), r"'a'\''b'");
}
