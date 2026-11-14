use super::{Regex, is_whitespace};

type TestResult = Result<(), String>;

/// `text.replace(new RegExp(source, flags), replacement)` in JavaScript, with `flags` `g` or `gi`.
fn replaced(source: &str, ignore_case: bool, text: &str, replacement: &str) -> Result<String, String> {
    let regex = Regex::new(source, ignore_case).ok_or_else(|| format!("{source} did not compile"))?;
    Ok(regex.replace_all(text, replacement))
}

fn check(source: &str, text: &str, replacement: &str, expected: &str) -> TestResult {
    let actual = replaced(source, false, text, replacement)?;
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "/{source}/g on {text:?}: expected {expected:?}, got {actual:?}"
        ))
    }
}

#[test]
fn literals_and_alternation_replace_every_match() -> TestResult {
    check("cat|dog", "a cat and a dog", "X", "a X and a X")?;
    check("", "ab", "-", "-a-b-")
}

#[test]
fn an_empty_match_moves_on_by_one_unit() -> TestResult {
    check("x*", "abc", "-", "-a-b-c-")
}

#[test]
fn case_folding_is_ascii_only() -> TestResult {
    assert_eq!(replaced("bearer", true, "BEARER bearer", "X")?, "X X");
    assert_eq!(replaced("bearer", false, "BEARER bearer", "X")?, "BEARER X");
    assert_eq!(replaced("[a-c]+", true, "ABCabcD", "X")?, "XD");
    assert_eq!(replaced("[^a-c]", true, "aBd", "X")?, "aBX");
    // The Kelvin sign and the long s fold onto ASCII letters only with the `u` flag.
    assert_eq!(replaced("k", true, "\u{212a}", "X")?, "\u{212a}");
    assert_eq!(replaced("s", true, "\u{17f}", "X")?, "\u{17f}");
    Ok(())
}

#[test]
fn whitespace_is_the_javascript_set() -> TestResult {
    check(r"\s+", "a\u{a0}\u{feff}\u{2003}b\u{85}c", "-", "a-b\u{85}c")?;
    for character in [' ', '\t', '\n', '\u{a0}', '\u{feff}', '\u{2028}', '\u{3000}'] {
        assert!(is_whitespace(character), "{character:?}");
    }
    for character in ['\u{85}', '\u{180e}', 'a', '\u{1f600}'] {
        assert!(!is_whitespace(character), "{character:?}");
    }
    Ok(())
}

#[test]
fn a_word_boundary_is_ascii() -> TestResult {
    check(r"\bab", "ab xab _ab ab", "X", "X xab _ab X")?;
    check(r"ab\b", "abab ab", "X", "abX X")?;
    check(r"\bab", "\u{e9}ab", "X", "\u{e9}X")
}

#[test]
fn quantifiers_are_greedy_unless_lazy() -> TestResult {
    check("a{2}", "aaaaa", "X", "XXa")?;
    check("a{2,}", "aaaaab", "X", "Xb")?;
    check("a{1,2}", "aaaaa", "X", "XXX")?;
    check("a+?", "aaa", "X", "XXX")?;
    check("a{2,3}?", "aaaaa", "X", "XXa")?;
    check("ba*?", "baaa", "X", "Xaaa")?;
    check("colou?r", "color colour", "X", "X X")?;
    check("colou??r", "color colour", "X", "X X")?;
    check("<[^>]*>", "a<b>c<d>", "X", "aXcX")?;
    check("a{3}", "aa", "X", "aa")
}

#[test]
fn a_group_repeats_with_backtracking() -> TestResult {
    check("(?:ab)+", "ababab ab", "X", "X X")?;
    check("(?:ab){2}", "ababab", "X", "Xab")?;
    check("(?:ab)*?c", "ababc", "X", "X")?;
    check("(?:ab)?a", "aba", "X", "X")?;
    check("(?:ab)??a", "aba", "X", "XbX")?;
    check("(?:a|ab)(?:c|bcd)", "abcd", "X", "X")?;
    check("(?:ab){2,3}b", "abababb", "X", "X")
}

#[test]
fn an_empty_iteration_ends_a_loop() -> TestResult {
    check("(?:a*)*b", "aaac", "X", "aaac")?;
    check("(?:a*)*b", "aab", "X", "X")
}

#[test]
fn a_capture_group_is_reinserted_with_the_first_reference() -> TestResult {
    check("(x+)y", "xxy xy", "$1!", "xx! x!")?;
    check("(a)?b", "b", "$1-", "-")?;
    check("(a|ab)(c|bcd)", "abcd", "$1.", "a.")?;
    check("(^|-)a", "a-a", "$1X", "X-X")
}

#[test]
fn a_capture_is_undone_when_its_alternative_fails() -> TestResult {
    check("(a)b|ac", "ac", "$1Z", "Z")
}

#[test]
fn lookahead_and_lookbehind_do_not_consume() -> TestResult {
    check("a(?=b)", "ab ac", "X", "Xb ac")?;
    check("a(?!b)", "ab ac", "X", "ab Xc")?;
    check("(?<![a-z])x", "ax x", "X", "ax X")?;
    check("(?<![a-z])x", "x", "X", "X")
}

#[test]
fn a_caret_only_matches_at_the_start_of_the_text() -> TestResult {
    check("^a", "aaa", "X", "Xaa")?;
    check("^a", "aa", "X", "Xa")
}

#[test]
fn matching_works_on_utf16_code_units() -> TestResult {
    // The emoji is two units, and a negated class takes one unit at a time.
    check("[^a]", "\u{1f600}a", "-", "--a")?;
    check("a", "\u{1f600}a", "\u{e9}", "\u{1f600}\u{e9}")?;
    // A capture that holds half of a surrogate pair is put back whole.
    check("(^|[^a])b", "\u{1f600}b", "$1X", "\u{1f600}X")
}

#[test]
fn unsupported_or_malformed_patterns_do_not_compile() {
    for source in [
        "(a",
        "(?:a",
        "[a",
        "[a-",
        "a{",
        "a{1",
        "a{1,",
        "a{x}",
        "a{99999999999999999999999}",
        "\\",
        ")",
        "a)",
        "(?<!",
    ] {
        assert!(Regex::new(source, false).is_none(), "{source} compiled");
    }
}
