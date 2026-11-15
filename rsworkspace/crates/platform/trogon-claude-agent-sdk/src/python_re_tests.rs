use super::*;
use crate::python_json_load::python_json_loads;
use crate::python_json_value::PythonJsonValue;
use serde_json::Value;

type TestResult = Result<(), Box<dyn std::error::Error>>;

// The entry points on `&str`, with each error as text, for the cases that hold no lone surrogate. These shadow the
// glob import.

fn error_text(error: PythonString) -> String {
    error.into_string().unwrap_or_else(|error| format!("{error:?}"))
}

fn search(pattern: &str, text: &str, budget: usize) -> Result<bool, String> {
    super::search(&pattern.into(), &text.into(), budget).map_err(error_text)
}

fn compiles(pattern: &str, budget: usize) -> Result<bool, String> {
    super::compiles(&pattern.into(), budget).map_err(error_text)
}

fn parse(pattern: &str, budget: usize) -> Result<Parsed, String> {
    super::parse(&pattern.into(), budget).map_err(error_text)
}

fn compile(parsed: &Parsed) -> Result<Program, String> {
    super::compile(parsed).map_err(error_text)
}

fn py_repr(text: &str) -> String {
    super::py_repr(&text.into())
}

fn is_re_error(text: &str) -> bool {
    super::is_re_error(&text.into())
}

/// The frame budget below `re.search` or `re.compile` called at module level, as the fixture was recorded: the
/// interpreter limit of 1000 minus the module frame and the `re.search` frame itself.
const MODULE_BUDGET: usize = 998;

/// Generated against CPython 3.12 `re` (`search` verdicts or `str(exception)`, and the compiled program).
const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/python_re.json");

fn text_of<'a>(case: &'a Value, name: &str) -> Result<&'a str, String> {
    case.get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing {name}"))
}

fn list<'a>(fixture: &'a Value, name: &str) -> Result<&'a Vec<Value>, String> {
    fixture
        .get(name)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing {name}"))
}

fn expected(case: &Value) -> Result<Result<bool, String>, String> {
    if let Some(error) = case.get("error").and_then(Value::as_str) {
        return Ok(Err(error.to_owned()));
    }
    case.get("match")
        .and_then(Value::as_bool)
        .map(Ok)
        .ok_or_else(|| "neither match nor error".to_owned())
}

#[test]
fn replays_every_python_search_case() -> TestResult {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    let cases = list(&fixture, "search")?;
    assert!(cases.len() >= 1500);
    let mut errors = 0;
    let mut mismatches = Vec::new();
    for case in cases {
        let (pattern, text) = (text_of(case, "pattern")?, text_of(case, "text")?);
        let want = expected(case)?;
        errors += usize::from(want.is_err());
        let got = search(pattern, text, MODULE_BUDGET);
        if got != want {
            mismatches.push(format!("{pattern:?} on {text:?}: got {got:?}, want {want:?}"));
        }
    }
    assert!(errors >= 300);
    assert!(
        mismatches.is_empty(),
        "{} mismatches, first: {:?}",
        mismatches.len(),
        mismatches.first()
    );
    Ok(())
}

#[test]
fn the_public_entry_point_agrees_with_the_inline_search() -> TestResult {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    for case in list(&fixture, "search")?.iter().step_by(97) {
        let (pattern, text) = (text_of(case, "pattern")?, text_of(case, "text")?);
        assert_eq!(
            search(pattern, text, MODULE_BUDGET),
            expected(case)?,
            "{pattern:?} on {text:?}"
        );
    }
    Ok(())
}

#[test]
fn replays_every_compiled_program() -> TestResult {
    let fixture: Value = serde_json::from_str(FIXTURE)?;
    let records = list(&fixture, "compile")?;
    assert!(records.len() >= 1000);
    let mut mismatches = Vec::new();
    for record in records {
        let pattern = text_of(record, "pattern")?;
        let compiled = parse(pattern, MODULE_BUDGET).and_then(|parsed| compile(&parsed));
        match (record.get("error").and_then(Value::as_str), compiled) {
            (Some(error), Err(got)) => {
                if got != error {
                    mismatches.push(format!("{pattern:?}: error {got:?}, want {error:?}"));
                }
            }
            (None, Ok(program)) => {
                let code: Vec<u64> = list(record, "code")?.iter().filter_map(Value::as_u64).collect();
                let got: Vec<u64> = program.code.iter().map(|word| u64::from(*word)).collect();
                let min = record.get("min").and_then(Value::as_u64);
                let groups = record.get("groups").and_then(Value::as_u64);
                if got != code || min != Some(u64::from(program.min_width)) || groups != Some(program.groups as u64) {
                    mismatches.push(format!(
                        "{pattern:?}: program differs (min {}, groups {})",
                        program.min_width, program.groups
                    ));
                }
            }
            (want, got) => mismatches.push(format!("{pattern:?}: want {want:?}, got ok={}", got.is_ok())),
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} mismatches, first: {:?}",
        mismatches.len(),
        mismatches.first()
    );
    Ok(())
}

#[test]
fn multi_line_patterns_report_the_line_and_column() {
    assert_eq!(
        search("a\n(", "", MODULE_BUDGET),
        Err("missing ), unterminated subpattern at position 2 (line 2, column 1)".to_owned())
    );
    assert_eq!(
        search("(", "", MODULE_BUDGET),
        Err("missing ), unterminated subpattern at position 0".to_owned())
    );
}

#[test]
fn matches_follow_python_search_semantics() {
    assert_eq!(search("a$", "a\n", MODULE_BUDGET), Ok(true));
    assert_eq!(search("a\\Z", "a\n", MODULE_BUDGET), Ok(false));
    assert_eq!(search("(?i)\u{212a}", "k", MODULE_BUDGET), Ok(true));
    assert_eq!(search("(?ia)\u{212a}", "k", MODULE_BUDGET), Ok(false));
    assert_eq!(search("(a)?(?(1)b|c)", "c", MODULE_BUDGET), Ok(true));
    assert_eq!(search("(?<=a)b", "ab", MODULE_BUDGET), Ok(true));
    assert_eq!(search("(?>a*)a", "aaa", MODULE_BUDGET), Ok(false));
    assert_eq!(search("a*+a", "aaa", MODULE_BUDGET), Ok(false));
    assert_eq!(search("", "", MODULE_BUDGET), Ok(true));
    assert_eq!(search("abc", "ab", MODULE_BUDGET), Ok(false));
}

#[test]
fn repr_quotes_and_escapes_like_python() {
    assert_eq!(py_repr("abc"), "'abc'");
    assert_eq!(py_repr("it's"), "\"it's\"");
    assert_eq!(py_repr("it's \"x\""), "'it\\'s \"x\"'");
    assert_eq!(py_repr("a\\b\tc\nd\re"), "'a\\\\b\\tc\\nd\\re'");
    assert_eq!(py_repr("\u{1}\u{7f}"), "'\\x01\\x7f'");
    assert_eq!(py_repr("\u{e9}"), "'\u{e9}'");
    assert_eq!(py_repr("\u{80}\u{ad}"), "'\\x80\\xad'");
    assert_eq!(py_repr("\u{2028}"), "'\\u2028'");
    assert_eq!(py_repr("\u{e0000}"), "'\\U000e0000'");
    assert_eq!(py_repr("\u{1f600}"), "'\u{1f600}'");
}

#[test]
fn identifiers_follow_str_isidentifier() {
    assert!(is_identifier("name_1"));
    assert!(is_identifier("\u{e9}t\u{e9}"));
    assert!(!is_identifier("1name"));
    assert!(!is_identifier(""));
    assert!(!is_identifier("a b"));
    assert!(!is_identifier("\u{2603}"));
}

#[test]
fn character_names_resolve_like_unicodedata() {
    assert_eq!(lookup_name("LATIN SMALL LETTER A"), Some(0x61));
    assert_eq!(lookup_name("latin small letter a"), Some(0x61));
    assert_eq!(lookup_name("LF"), Some(0x0a));
    assert_eq!(lookup_name("HANGUL SYLLABLE GA"), Some(0xac00));
    assert_eq!(lookup_name("HANGUL SYLLABLE XYZ"), None);
    assert_eq!(lookup_name("CJK UNIFIED IDEOGRAPH-65E5"), Some(0x65e5));
    assert_eq!(lookup_name("CJK UNIFIED IDEOGRAPH-0041"), None);
    assert_eq!(lookup_name("NOT A CHARACTER"), None);
}

#[test]
fn folding_modes_pick_their_case_tables() {
    assert_eq!(Fold::of(0), Fold::Off);
    assert_eq!(Fold::of(FLAG_IGNORECASE | FLAG_ASCII), Fold::Ascii);
    assert_eq!(Fold::of(FLAG_IGNORECASE | FLAG_UNICODE), Fold::Unicode);
    assert!(!Fold::Off.iscased(u32::from('a')));
    assert_eq!(Fold::Off.lower(u32::from('A')), u32::from('A'));
    assert_eq!(Fold::Ascii.lower(u32::from('A')), u32::from('a'));
    assert_eq!(Fold::Ascii.lower(0xc9), 0xc9);
    assert_eq!(Fold::Unicode.lower(0xc9), 0xe9);
    assert!(Fold::Ascii.iscased(u32::from('a')));
    assert!(Fold::Unicode.iscased(0xe9));
    assert!(Fold::Off.fixes(u32::from('s')).is_none());
    assert!(Fold::Ascii.fixes(u32::from('s')).is_none());
    assert!(Fold::Unicode.fixes(u32::from('s')).is_some());
}

#[test]
fn repeat_kinds_carry_their_constant_names() {
    assert_eq!(RepeatKind::Max.name(), "MAX_REPEAT");
    assert_eq!(RepeatKind::Min.name(), "MIN_REPEAT");
    assert_eq!(RepeatKind::Possessive.name(), "POSSESSIVE_REPEAT");
}

#[test]
fn nesting_past_the_parser_frame_limit_is_a_recursion_error() {
    let shallow = format!("{}a{}", "(".repeat(400), ")".repeat(400));
    assert_eq!(search(&shallow, "a", MODULE_BUDGET), Ok(true));
    let deep = format!("{}a{}", "(".repeat(700), ")".repeat(700));
    assert_eq!(search(&deep, "a", MODULE_BUDGET), Err(RECURSION_ERROR.to_owned()));
}

/// The deepest frame below `re.compile` each pattern reaches, measured with a CPython 3.12 profile hook: the
/// compile succeeds with exactly that budget and raises `RecursionError` with one frame less.
const FRAME_COSTS: [(&str, usize); 25] = [
    ("", 6),
    ("a", 7),
    ("a|b", 7),
    ("[1]", 7),
    ("[\\d]", 7),
    ("[\\1]", 7),
    ("\\0", 7),
    ("\\00", 8),
    ("\\101", 8),
    ("\\x41", 8),
    ("\\u0041", 8),
    ("\\U00000041", 8),
    ("[\\x41]", 8),
    ("[\\101]", 8),
    ("(?i)a", 8),
    ("(?x)a #c", 8),
    ("(a)", 9),
    ("(?:a)", 9),
    ("(?i:a)", 9),
    ("(a)\\1", 9),
    ("(?P<n>a)(?P=n)", 9),
    ("((a))", 11),
    ("((()))", 12),
    ("(a)(?(1)(?(1)(?(1)(?(1)(?(1)a)))))", 12),
    ("(?=(?=(?=(?=(?=a)))))", 17),
];

#[test]
fn every_parser_frame_counts_against_the_budget() {
    for (pattern, cost) in FRAME_COSTS {
        assert_eq!(compiles(pattern, cost), Ok(true), "{pattern:?} at {cost}");
        assert_eq!(
            compiles(pattern, cost - 1),
            Err(RECURSION_ERROR.to_owned()),
            "{pattern:?} at {}",
            cost - 1
        );
    }
    // The tokenizer reads its first token before any syntax error can surface.
    assert_eq!(compiles("\\", 4), Err(RECURSION_ERROR.to_owned()));
    assert_eq!(compiles("\\", MODULE_BUDGET), Ok(false));
    assert_eq!(search("a", "a", 6), Err(RECURSION_ERROR.to_owned()));
}

/// Syntax errors and group references, measured as for [`FRAME_COSTS`]: with exactly that budget the compile raises
/// the `re.error` shown (or succeeds for `None`), and with one frame less the `re.error` constructor, the
/// `source.error` call or the `State.checkgroup` call that reaches deepest raises `RecursionError` instead.
const ERROR_FRAME_COSTS: [(&str, usize, Option<&str>); 50] = [
    ("\\", 6, Some("bad escape (end of pattern) at position 0")),
    ("a\\", 8, Some("bad escape (end of pattern) at position 1")),
    ("(|\\", 9, Some("bad escape (end of pattern) at position 2")),
    ("a|\\", 7, Some("bad escape (end of pattern) at position 2")),
    ("[\\", 8, Some("bad escape (end of pattern) at position 1")),
    ("(?<=a\\", 10, Some("bad escape (end of pattern) at position 5")),
    ("(?i\\", 8, Some("bad escape (end of pattern) at position 3")),
    ("\\q", 8, Some("bad escape \\q at position 0")),
    ("(\\q)", 10, Some("bad escape \\q at position 1")),
    ("((((\\q))))", 16, Some("bad escape \\q at position 4")),
    ("[\\q]", 8, Some("bad escape \\q at position 1")),
    ("([\\q])", 10, Some("bad escape \\q at position 2")),
    ("\\N", 8, Some("missing { at position 2")),
    ("\\N{", 9, Some("missing character name at position 3")),
    ("(\\N{", 11, Some("missing character name at position 4")),
    ("\\N{XYZ}", 8, Some("undefined character name 'XYZ' at position 0")),
    ("\\x", 8, Some("incomplete escape \\x at position 0")),
    ("(\\x)", 10, Some("incomplete escape \\x at position 1")),
    (
        "\\777",
        8,
        Some("octal escape value \\777 outside of range 0-0o377 at position 0"),
    ),
    (
        "(\\777)",
        10,
        Some("octal escape value \\777 outside of range 0-0o377 at position 1"),
    ),
    ("\\8", 8, Some("invalid group reference 8 at position 1")),
    ("(\\8)", 10, Some("invalid group reference 8 at position 2")),
    ("(?P<", 8, Some("missing group name at position 4")),
    ("(?P<1>a)", 8, Some("bad character in group name '1' at position 4")),
    ("((?P<1>a))", 10, Some("bad character in group name '1' at position 5")),
    ("(?(1", 8, Some("missing ), unterminated name at position 3")),
    ("(?(a)b)", 7, Some("unknown group name 'a' at position 3")),
    ("(?P=n)", 7, Some("unknown group name 'n' at position 4")),
    (
        "(?P<n>a)(?P<n>b)",
        9,
        Some("redefinition of group name 'n' as group 2; was group 1 at position 12"),
    ),
    ("(", 8, Some("missing ), unterminated subpattern at position 0")),
    ("((", 10, Some("missing ), unterminated subpattern at position 1")),
    ("a)", 7, Some("unbalanced parenthesis at position 1")),
    ("(?(1)a)", 8, Some("invalid group reference 1 at position 3")),
    (
        "(?L)",
        8,
        Some("bad inline flags: cannot use 'L' flag with a str pattern at position 3"),
    ),
    (
        "a(?i)",
        8,
        Some("global flags not at the start of the expression at position 1"),
    ),
    ("[a-\\d]", 7, Some("bad character range a-\\d at position 1")),
    ("(a)(\\1)", 10, None),
    ("(a)((\\1))", 12, None),
    ("(a)(?<=\\1)", 11, None),
    ("(a)(?<=\\1\\1)", 11, None),
    (
        "(?<=(a)\\1)",
        11,
        Some("cannot refer to group defined in the same lookbehind subpattern at position 9"),
    ),
    ("(?<=\\1(a))", 10, Some("invalid group reference 1 at position 5")),
    ("(a(?<=\\1))", 12, Some("cannot refer to an open group at position 6")),
    ("(?<=a(?<=b\\1))", 12, Some("invalid group reference 1 at position 11")),
    ("(?P<n>a)(?<=(?P=n))", 10, None),
    (
        "(?P<n>a)(?<=(?P<m>b)(?P=m))",
        11,
        Some("cannot refer to group defined in the same lookbehind subpattern at position 26"),
    ),
    ("(a)(?<=(?(1)b|c))", 10, None),
    (
        "(?<=(a)(?(1)b|c))",
        11,
        Some("cannot refer to group defined in the same lookbehind subpattern at position 12"),
    ),
    ("(?P<n>a)((?P=n))", 9, None),
    ("(a)(?<=(\\1))", 13, None),
];

#[test]
fn building_the_re_error_and_checking_groups_count_against_the_budget() {
    for (pattern, cost, error) in ERROR_FRAME_COSTS {
        let compiled = parse(pattern, cost).and_then(|parsed| compile(&parsed)).map(drop);
        assert_eq!(
            compiled,
            error.map_or(Ok(()), |text| Err(text.to_owned())),
            "{pattern:?} at {cost}"
        );
        assert_eq!(
            compiles(pattern, cost - 1),
            Err(RECURSION_ERROR.to_owned()),
            "{pattern:?} at {}",
            cost - 1
        );
    }
}

/// `re.compile` as the `regex` format check calls it: `re.error` reads as a pattern that does not compile, and
/// the `OverflowError`, `ValueError` and `RecursionError` CPython 3.12 raises instead stay exceptions.
#[test]
fn compiling_separates_re_errors_from_other_exceptions() {
    assert_eq!(compiles("a(b)", MODULE_BUDGET), Ok(true));
    assert_eq!(compiles("(", MODULE_BUDGET), Ok(false));
    assert_eq!(compiles("(?<=a*)b", MODULE_BUDGET), Ok(false));
    assert_eq!(
        compiles("a{99999999999}", MODULE_BUDGET),
        Err(REPEAT_TOO_LARGE.to_owned())
    );
    let digits = compiles(&format!("a{{{}}}", "1".repeat(5000)), MODULE_BUDGET);
    assert!(digits.is_err_and(|text| text.starts_with(DIGIT_LIMIT_PREFIX)));
    let deep = format!("{}a{}", "(".repeat(700), ")".repeat(700));
    assert_eq!(compiles(&deep, MODULE_BUDGET), Err(RECURSION_ERROR.to_owned()));
    assert!(!is_re_error(FLAGS_INCOMPATIBLE));
    assert!(is_re_error("look-behind requires fixed-width pattern"));
}

/// `re.search` on patterns and texts holding lone surrogates, recorded from CPython 3.12: a surrogate is a literal
/// like any other code point, and the messages that quote pattern text quote it as it is, or through `repr`.
#[test]
fn lone_surrogates_match_and_fail_as_python_does() -> TestResult {
    let fixture = python_json_loads(include_str!("../tests/fixtures/sdk_mcp/python_re_surrogates.json"))?;
    let cases = fixture
        .get("search")
        .and_then(PythonJsonValue::as_array)
        .ok_or("missing search")?;
    assert_eq!(cases.len(), 26);
    for case in cases {
        let field = |name: &str| case.get(name).and_then(PythonJsonValue::as_python_str);
        let (pattern, text) = (
            field("pattern").ok_or("missing pattern")?,
            field("text").ok_or("missing text")?,
        );
        let want = match field("error") {
            Some(error) => Err(error.clone()),
            None => Ok(case
                .get("match")
                .and_then(PythonJsonValue::as_bool)
                .ok_or("missing match")?),
        };
        assert_eq!(
            super::search(pattern, text, MODULE_BUDGET),
            want,
            "{pattern:?} on {text:?}"
        );
    }
    Ok(())
}

// Lines that no test can reach, and why:
// - `Fold::Off` branches that CPython never takes with a fold-only opcode are kept for exhaustive matching.

/// CPython 3.12 `re.search` verdicts, or `str(re.error)`, for flag groups, sets that fold past the first 256 code
/// points, sets wide enough for a `BIGCHARSET`, references inside alternations and look-behinds.
#[test]
fn sets_flags_and_references_search_as_cpython_does() {
    let cases: [(&str, &str, Result<bool, &str>); 24] = [
        ("(a)(?:\\1b|\\1c)", "aac", Ok(true)),
        ("(a)(?:\\1b|\\1c)", "aad", Ok(false)),
        (
            "(a(?<=(?(1)b)))",
            "",
            Err("cannot refer to an open group at position 11"),
        ),
        ("(?i1)", "", Err("missing -, : or ) at position 3")),
        ("(?-iz:)", "", Err("unknown flag at position 4")),
        ("(?-i1:)", "", Err("missing : at position 4")),
        ("(?", "", Err("unexpected end of pattern at position 2")),
        ("(?i)[kx]", "\u{212a}", Ok(true)),
        ("(?i)[j-l]", "\u{212a}", Ok(true)),
        ("(?i)[1\u{10400}]", "\u{10428}", Ok(true)),
        ("(?i)[a\u{1f600}]", "\u{1f600}", Ok(true)),
        ("(?i)[^k]", "\u{212a}", Ok(false)),
        ("(?i)[^k]", "x", Ok(true)),
        ("(?<=a{4294967294}a{4294967294})", "", Err("looks too much behind")),
        ("(?i)[\u{10400}-\u{10427}]", "\u{10428}", Ok(true)),
        ("[\u{100}\u{102}\u{104}]", "\u{102}", Ok(true)),
        ("[\u{100}\u{102}\u{104}]", "\u{101}", Ok(false)),
        ("(?i)[ace]", "C", Ok(true)),
        ("(?i)[sx]", "\u{17f}", Ok(true)),
        ("(?i)[r-t]", "\u{17f}", Ok(true)),
        ("(?i)[^s]", "\u{17f}", Ok(false)),
        ("(?i)[^s]", "x", Ok(true)),
        ("(?i)(?-im:a)", "A", Ok(false)),
        ("(?i)(?-im:a)", "a", Ok(true)),
    ];
    for (pattern, text, expected) in cases {
        assert_eq!(
            search(pattern, text, MODULE_BUDGET),
            expected.map_err(str::to_owned),
            "{}",
            py_repr(pattern)
        );
    }
}
