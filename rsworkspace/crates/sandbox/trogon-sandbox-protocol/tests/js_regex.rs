use std::ops::Range;

use trogon_sandbox_contract::ExtractPattern;
use trogon_sandbox_protocol::{GroupOneUndefinedError, JsRegex};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Spans = Result<Vec<Range<usize>>, GroupOneUndefinedError>;

fn spans(pattern: &str, content: &str) -> Result<Spans, Box<dyn std::error::Error>> {
    Ok(JsRegex::new(&ExtractPattern::new(pattern)?)?.group_one_spans(content))
}

#[test]
fn constructs_only_javascript_accepts_match_as_the_engine_does() -> TestResult {
    type Case<'a> = (&'a str, &'a str, &'a str, &'a [(usize, usize)]);
    let cases: &[Case<'_>] = &[
        ("lookbehind", r"(?<=token=)(\w+)", "token=abc", &[(6, 9)]),
        ("lookahead_capture", r"token=(?=(\w+))", "token=abc", &[(6, 9)]),
        ("negative_lookahead", "(a)(?!b)", "abac", &[(2, 3)]),
        ("negative_lookbehind", "(?<!x)(a)", "xaba", &[(3, 4)]),
        ("numbered_backreference", r"(a)\1", "aaab", &[(0, 1)]),
        ("named_backreference", r"(?<n>a)\k<n>", "aab", &[(0, 1)]),
        ("legacy_octal_escape", r"(a)\2", "a\u{2}", &[(0, 1)]),
        ("identity_escape", r"\q(a)", "qa", &[(1, 2)]),
        ("literal_brace", "(a){,2}", "a{,2}", &[(0, 1)]),
        ("unclosed_brace_is_literal", "x{2(b)", "x{2b", &[(3, 4)]),
        ("brace_unicode_escape_is_identity_u", r"\u{2}(b)", "uub", &[(2, 3)]),
        ("brace_unicode_escape_in_class", r"[\u{41}](a)", "ua", &[(1, 2)]),
        ("modifier_group", "(?i:a)(b)", "Ab", &[(1, 2)]),
        ("empty_group_is_skipped", "(a*)b", "bab", &[(1, 2)]),
        ("lazy_quantifier", "(a+?)", "aaa", &[(0, 1), (1, 2), (2, 3)]),
        ("quantified_lookahead", "(?=a)*(b)", "b", &[(0, 1)]),
        ("range_with_class_escape", r"[\d-z](a)", "-a", &[(1, 2)]),
        ("control_escape", r"\cA(a)", "\u{1}a", &[(1, 2)]),
        ("dollar_group", r"(\$\w+)", "x $abc y", &[(2, 6)]),
        ("dot_excludes_newline", "(.)", "a\nb", &[(0, 1), (2, 3)]),
    ];
    for (name, pattern, content, expected) in cases {
        let got: Vec<(usize, usize)> = spans(pattern, content)?
            .map_err(|error| format!("{name}: {error}"))?
            .into_iter()
            .map(|range| (range.start, range.end))
            .collect();
        assert_eq!(&got, expected, "{name}");
    }
    Ok(())
}

#[test]
fn an_alternative_that_leaves_group_one_unset_aborts_the_scan() -> TestResult {
    assert_eq!(spans("(a)|b", "ab")?, Err(GroupOneUndefinedError { offset: 1 }));
    assert_eq!(
        spans("(?<n>a)|(?<n>b)", "ab")?,
        Err(GroupOneUndefinedError { offset: 1 })
    );
    assert_eq!(spans(r"(a)?\1(b)", "b")?, Err(GroupOneUndefinedError { offset: 0 }));
    assert_eq!(
        GroupOneUndefinedError { offset: 3 }.to_string(),
        "capture group 1 is undefined in the match at byte 3"
    );
    Ok(())
}

#[test]
#[allow(clippy::single_range_in_vec_init)]
fn surrogate_escapes_match_the_code_units_the_engine_matches() -> TestResult {
    assert_eq!(spans(r"😀(a)", "😀a")?, Ok(vec![4..5]));
    assert_eq!(spans(r"[😀](a)", "😀a")?, Ok(vec![4..5]));
    assert_eq!(spans("😀(a)", "😀a")?, Ok(vec![4..5]));
    Ok(())
}

#[test]
#[allow(clippy::single_range_in_vec_init)]
fn a_group_that_splits_a_surrogate_pair_widens_to_the_whole_scalar() -> TestResult {
    assert_eq!(spans("(.)", "😀")?, Ok(vec![0..4]));
    assert_eq!(spans("(..)", "a😀")?, Ok(vec![0..5]));
    assert_eq!(spans("[^a](.)", "😀b")?, Ok(vec![0..4]));
    Ok(())
}

#[test]
#[allow(clippy::single_range_in_vec_init)]
fn overlapping_spans_merge() -> TestResult {
    assert_eq!(spans("(?=(aa))a", "aaa")?, Ok(vec![0..3]));
    Ok(())
}

#[test]
fn duplicate_names_are_allowed_only_in_different_alternatives() {
    assert!(ExtractPattern::new("(?<n>a)|(?<n>b)").is_ok());
    assert!(ExtractPattern::new("(?:(?<n>a)|b)(?<n>c)").is_err());
}
