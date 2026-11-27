use std::time::{Duration, Instant};

use trogon_sandbox_contract::{GlobPattern, LiteralPath};
use trogon_sandbox_protocol::{GlobMatcher, GlobToken, LiteralPathExt, parse_glob, textual_brackets};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn lit(raw: &str) -> Result<LiteralPath, Box<dyn std::error::Error>> {
    Ok(LiteralPath::new(raw)?)
}

fn matcher(raw: &str) -> Result<GlobMatcher, Box<dyn std::error::Error>> {
    Ok(GlobMatcher::new(&GlobPattern::new(raw)?))
}

fn t(text: &str) -> GlobToken {
    GlobToken::Text(text.to_string())
}

/// The ranges that `members` spell, read independently of the library.
fn ranges_of(members: &str) -> Vec<(u16, u16)> {
    let units: Vec<u16> = members.encode_utf16().collect();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < units.len() {
        let is_range = units.get(i + 1) == Some(&(b'-' as u16)) && i + 2 < units.len();
        let from = units[i];
        if is_range {
            i += 2;
        }
        ranges.push((from, units[i]));
        i += 1;
    }
    ranges
}

fn set(members: &str) -> GlobToken {
    GlobToken::Set(ranges_of(members))
}

const SEP: GlobToken = GlobToken::Separator;
const ANY: GlobToken = GlobToken::AnyChar;
const STAR: GlobToken = GlobToken::Star;
const GS: GlobToken = GlobToken::Globstar;
const GSS: GlobToken = GlobToken::GlobstarSlash;

fn check(rows: Vec<(&str, Vec<GlobToken>)>) {
    for (pattern, tokens) in rows {
        assert_eq!(parse_glob(pattern), tokens, "{pattern:?}");
    }
}

fn glob_matches(pattern: &str, path: &str) -> Result<bool, Box<dyn std::error::Error>> {
    Ok(matcher(pattern)?.matches(&lit(path)?))
}

#[test]
fn reads_plain_patterns() {
    check(vec![
        ("", vec![]),
        ("/a/b.ts", vec![SEP, t("a"), SEP, t("b.ts")]),
        ("*.t?", vec![STAR, t(".t"), ANY]),
        ("a/**/b", vec![t("a"), SEP, GSS, t("b")]),
        ("a/**", vec![t("a"), SEP, GS]),
        ("a**b", vec![t("a"), GS, t("b")]),
        ("\\(", vec![t("\\(")]),
    ]);
}

#[test]
fn gives_the_last_two_of_a_star_run_to_the_separator() {
    check(vec![
        ("*/", vec![STAR, SEP]),
        ("**/", vec![GSS]),
        ("***/", vec![STAR, GSS]),
        ("***", vec![GS]),
        ("****", vec![GS]),
    ]);
}

#[test]
fn keeps_the_wildcards_to_the_fewest() {
    check(vec![
        ("****/", vec![GS]),
        ("*****/", vec![GS]),
        ("**/**/", vec![GSS]),
        ("**/**", vec![GS]),
        ("***/**", vec![GS]),
        ("***/*", vec![GS]),
        ("**/***/", vec![GS]),
        ("***/***/***/", vec![GS]),
        ("a***/*b", vec![t("a"), GS, t("b")]),
    ]);
}

#[test]
fn keeps_wildcards_that_must_stay() {
    check(vec![
        ("**/*", vec![GSS, STAR]),
        ("*a*", vec![STAR, t("a"), STAR]),
        ("*?*", vec![STAR, ANY, STAR]),
        ("*/*", vec![STAR, SEP, STAR]),
        ("**/a**/", vec![GSS, t("a"), GSS]),
        ("**[a]**", vec![GS, set("a"), GS]),
    ]);
}

#[test]
fn reads_sets() {
    check(vec![
        ("[abc]", vec![set("abc")]),
        ("x[a-z]y", vec![t("x"), set("a-z"), t("y")]),
        ("[-a]", vec![set("-a")]),
        ("[a-]", vec![set("a-")]),
        ("[a-a]", vec![set("a-a")]),
        ("[^a]", vec![set("^a")]),
        ("[!a]", vec![set("!a")]),
        ("[a[b]", vec![set("a[b")]),
        ("[a]]", vec![set("a"), t("]")]),
        ("[s/]", vec![set("s/")]),
        ("[+-9]", vec![set("+-9")]),
        ("[0-9]", vec![set("0-9")]),
        ("[*]", vec![set("*")]),
        ("[a?]", vec![set("a?")]),
        ("[*-a]", vec![set("*-a")]),
    ]);
}

#[test]
fn reads_brackets_that_spell_no_set_as_text() {
    check(vec![
        ("[z-a]", vec![t("[z-a]")]),
        ("[user-id]", vec![t("[user-id]")]),
        ("[]", vec![t("[]")]),
        ("[]a]", vec![t("[]a]")]),
        ("a]b", vec![t("a]b")]),
        ("a[b", vec![t("a[b")]),
        ("a[b[c", vec![t("a[b[c")]),
        ("[[z-a]", vec![t("[[z-a]")]),
        ("[z-a[b]", vec![t("[z-a[b]")]),
        ("[z-a][b]", vec![t("[z-a]"), set("b")]),
        ("[\u{1f600}-\u{1f601}]", vec![t("[\u{1f600}-\u{1f601}]")]),
    ]);
}

#[test]
fn keeps_what_is_between_such_brackets_a_pattern() {
    check(vec![
        ("[z-a*]", vec![t("[z-a"), STAR, t("]")]),
        ("[z-a/?]", vec![t("[z-a"), SEP, ANY, t("]")]),
        ("a[b[c*", vec![t("a[b[c"), STAR]),
        ("[**/", vec![t("["), GSS]),
    ]);
}

#[test]
fn parses_a_million_characters_in_linear_time() {
    for unit in ["[", "[a", "[a]", "[z-a]", "[z-a[", "*", "?"] {
        let pattern = unit.repeat(1_000_000 / unit.len());
        let began = Instant::now();
        parse_glob(&pattern);
        assert!(began.elapsed() < Duration::from_secs(2), "{unit:?}");
        let began = Instant::now();
        textual_brackets(&pattern);
        assert!(began.elapsed() < Duration::from_secs(2), "{unit:?}");
    }
}

#[test]
fn brackets_that_spell_no_set_are_found() {
    assert_eq!(textual_brackets("/x/a[]b"), ["[]"]);
    assert_eq!(textual_brackets("/x/a[b"), ["[b"]);
    assert_eq!(textual_brackets("/x/[z-a]/[a]/["), ["[z-a]", "["]);
    assert!(textual_brackets("/x/a[bc]d").is_empty());
    assert!(textual_brackets("/x/plain").is_empty());
}

#[test]
fn the_dialect_matches_its_documented_tokens() -> TestResult {
    assert!(glob_matches("/d/*.ts", "/d/foo.ts")?);
    assert!(!glob_matches("/d/*.ts", "/d/a/foo.ts")?);
    assert!(glob_matches("/d/**/*.ts", "/d/foo.ts")?);
    assert!(glob_matches("/d/**/*.ts", "/d/a/b/foo.ts")?);
    assert!(glob_matches("/d/**", "/d/a/b")?);
    assert!(glob_matches("/d/file?.txt", "/d/file1.txt")?);
    assert!(!glob_matches("/d/file?.txt", "/d/file/.txt")?);
    assert!(glob_matches("/d/file[0-9].txt", "/d/file3.txt")?);
    assert!(!glob_matches("/d/file[0-9].txt", "/d/filex.txt")?);
    assert!(glob_matches("/d/[!a]", "/d/!")?);
    assert!(!glob_matches("/d/[^a]", "/d/b")?);
    assert!(glob_matches("/d/[^a]", "/d/^")?);
    Ok(())
}

#[test]
fn takes_a_wildcard_in_a_set_for_the_character() -> TestResult {
    for wildcard in ["*", "?"] {
        let pattern = format!("/tmp/a[{wildcard}]c");
        assert!(glob_matches(&pattern, &format!("/tmp/a{wildcard}c"))?);
        assert!(!glob_matches(&pattern, "/tmp/abc")?);
        assert!(!glob_matches(&pattern, "/tmp/ac")?);
        assert!(!glob_matches(&pattern, &format!("/tmp/a[{wildcard}]c"))?);
    }
    Ok(())
}

#[test]
fn counts_a_character_beyond_the_basic_plane_as_two_units() -> TestResult {
    assert!(!glob_matches("/w/?", "/w/\u{1f600}")?);
    assert!(glob_matches("/w/??", "/w/\u{1f600}")?);
    assert!(!glob_matches("/w/[\u{1f600}]", "/w/\u{1f600}")?);
    assert!(glob_matches("/w/*", "/w/\u{1f600}")?);
    assert!(glob_matches("/w/\u{1f600}?", "/w/\u{1f600}x")?);
    Ok(())
}

#[test]
fn takes_empty_brackets_for_text() -> TestResult {
    assert!(GlobPattern::new("/tmp/list[].ts").is_err());
    let matcher = GlobMatcher::new(&GlobPattern::new("/tmp/list[].t*")?);
    assert!(matcher.matches(&lit("/tmp/list[].ts")?));
    assert!(!matcher.matches(&lit("/tmp/list.ts")?));
    Ok(())
}

#[test]
fn takes_every_unclosed_bracket_for_text_before_a_wildcard_too() -> TestResult {
    assert!(glob_matches("/tmp/a[b[c*", "/tmp/a[b[c and more")?);
    assert!(!glob_matches("/tmp/a[b[c*", "/tmp/a[b[c/more")?);
    assert!(!glob_matches("/tmp/a[b[c*", "/tmp/a[bc")?);
    Ok(())
}

#[test]
fn answers_at_once_for_a_thousand_characters_of_wildcards() -> TestResult {
    for unit in ["*", "**/", "***/", "**/*"] {
        let pattern = format!("/x/{}y", unit.repeat(1_000 / unit.len()));
        let began = Instant::now();
        assert!(!glob_matches(&pattern, &format!("/x/{}z", "a/".repeat(40)))?);
        assert!(glob_matches(&pattern, &format!("/x/{}y", "a/".repeat(40)))?);
        assert!(began.elapsed() < Duration::from_secs(1), "{unit:?}");
    }
    Ok(())
}

#[test]
fn stays_tractable_on_adjacent_wildcards() -> TestResult {
    let pattern = matcher(&format!("/{}x", "*/".repeat(200)))?;
    assert!(!pattern.matches(&lit(&format!("/{}", "a/".repeat(300)))?));
    Ok(())
}

fn set_of(members: &[char]) -> Option<Vec<(char, char)>> {
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < members.len() {
        let is_range = members.get(i + 1) == Some(&'-') && i + 2 < members.len();
        ranges.push((members[i], members[if is_range { i + 2 } else { i }]));
        if is_range {
            i += 2;
        }
        i += 1;
    }
    if ranges.is_empty() || ranges.iter().any(|(from, to)| from > to) {
        return None;
    }
    Some(ranges)
}

fn from(pattern: &[char], p: usize, path: &[char], s: usize, text_until: usize) -> bool {
    if p == pattern.len() {
        return s == path.len();
    }
    let head = pattern[p];
    if head == '*' {
        let mut stars = 0;
        while pattern.get(p + stars) == Some(&'*') {
            stars += 1;
        }
        let directories = stars >= 2 && pattern.get(p + stars) == Some(&'/');
        let next = p + stars + usize::from(directories);
        if directories {
            stars -= 2;
        }
        for end in s..=path.len() {
            if end > s && stars == 0 {
                break;
            }
            if end > s && stars == 1 && path[end - 1] == '/' {
                break;
            }
            if from(pattern, next, path, end, text_until) {
                return true;
            }
            if !directories {
                continue;
            }
            for i in end..path.len() {
                if path[i] == '/' && from(pattern, next, path, i + 1, text_until) {
                    return true;
                }
            }
        }
        return false;
    }
    if s == path.len() {
        return false;
    }
    if head == '?' {
        return path[s] != '/' && from(pattern, p + 1, path, s + 1, text_until);
    }
    let mut text_until = text_until;
    if head == '[' && p >= text_until {
        let close = pattern[p + 1..].iter().position(|c| *c == ']').map(|o| p + 1 + o);
        let ranges = close.and_then(|close| set_of(&pattern[p + 1..close]));
        if let (Some(close), Some(ranges)) = (close, ranges) {
            return ranges.iter().any(|(a, b)| *a <= path[s] && path[s] <= *b)
                && from(pattern, close + 1, path, s + 1, text_until);
        }
        text_until = close.map_or(pattern.len(), |close| close + 1);
    }
    path[s] == head && from(pattern, p + 1, path, s + 1, text_until)
}

struct Xorshift(u64);

impl Xorshift {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n as u64) as usize
    }
}

#[test]
fn agrees_with_a_reference_matcher_over_generated_patterns_and_paths() -> TestResult {
    let mut pieces: Vec<String> = "[[]]--**??//az09+.^!\\".chars().map(String::from).collect();
    pieces.extend(["**/", "[a-z]", "[z-a]", "[+-9]", "[*]", "[]"].map(String::from));
    let chars: Vec<char> = "[]-*?/az09+.^!\\".chars().collect();
    let mut rng = Xorshift(0x2026_1015_9e37_79b9);
    let mut compared = 0;
    let mut matched = 0;
    for _ in 0..40_000 {
        let pattern: String = (0..rng.below(9))
            .map(|_| pieces[rng.below(pieces.len())].clone())
            .collect();
        let path: String = if rng.below(2) == 0 {
            (0..rng.below(7)).map(|_| chars[rng.below(chars.len())]).collect()
        } else {
            pattern
                .chars()
                .map(|c| {
                    if rng.below(2) == 0 {
                        c
                    } else {
                        chars[rng.below(chars.len())]
                    }
                })
                .collect()
        };
        let rooted_pattern = format!("/{pattern}");
        let rooted_path = format!("/{path}");
        let (Ok(glob), Ok(literal)) = (GlobPattern::new(&rooted_pattern), LiteralPath::new(&rooted_path)) else {
            continue;
        };
        if glob.as_str() != rooted_pattern || literal.as_str() != rooted_path {
            continue;
        }
        let p: Vec<char> = rooted_pattern.chars().collect();
        let f: Vec<char> = rooted_path.chars().collect();
        let expected = from(&p, 0, &f, 0, 0);
        compared += 1;
        if expected {
            matched += 1;
        }
        assert_eq!(
            GlobMatcher::new(&glob).matches(&literal),
            expected,
            "{rooted_pattern:?} against {rooted_path:?}"
        );
    }
    assert!(compared > 2_000, "{compared}");
    assert!(matched > 100, "{matched}");
    Ok(())
}

#[test]
fn glob_directory_form_is_the_directory_before_a_trailing_globstar() -> TestResult {
    let pattern = matcher("/w/**/build/**")?;
    assert!(pattern.matches_directory_form(&lit("/w/a/build")?));
    assert!(pattern.matches_directory_form(&lit("/w/build")?));
    assert!(!pattern.matches_directory_form(&lit("/w/a/dist")?));
    let inner = matcher("/w/**/*.out")?;
    assert!(!inner.matches_directory_form(&lit("/w/a/b.out")?));
    Ok(())
}

#[test]
fn glob_matching_can_ignore_case() -> TestResult {
    let pattern = matcher("/C:/Work/*.KEY")?;
    let path = lit("/c:/work/a.key")?;
    assert!(!pattern.matches(&path));
    assert!(pattern.matches_ignoring_case(&path));
    let set = matcher("/C:/[A-C]x")?;
    assert!(set.matches_ignoring_case(&lit("/c:/bx")?));
    assert!(!set.matches_ignoring_case(&lit("/c:/dx")?));
    Ok(())
}

#[test]
fn a_pattern_covers_what_lies_beneath_a_match() -> TestResult {
    let glob = matcher("/data/*")?;
    assert!(glob.covers(&lit("/data/a")?));
    assert!(glob.covers(&lit("/data/a/deep/file")?));
    assert!(!glob.covers(&lit("/other/a")?));
    Ok(())
}

#[test]
fn a_glob_cursor_agrees_with_the_matcher_and_dies_off_the_pattern() -> TestResult {
    let patterns = [
        "/w/*.txt",
        "/w/**/x",
        "/w/a?c",
        "/w/[a-c]x/**",
        "/w/proj/*.pem",
        "/w/**/dir/**/f*",
        "/w/a**",
        "/w/a[b/c]d/e",
    ];
    let paths = [
        "/w/a.txt",
        "/w/x",
        "/w/q/x",
        "/w/abc",
        "/w/bx",
        "/w/bx/y/z",
        "/w/proj/a.pem",
        "/w/proj/a/b.pem",
        "/w/d/dir/e/fz",
        "/w/ab/c",
        "/w/a/d/e",
        "/w/abd/e",
        "/w",
    ];
    for pattern in patterns {
        let glob = matcher(pattern)?;
        let base = glob.static_base()?;
        for path in paths {
            let Some(rest) = path.strip_prefix(base.as_str()) else {
                continue;
            };
            let mut cursor = glob.cursor(&base);
            for name in rest.split('/').filter(|n| !n.is_empty()) {
                cursor = cursor.child(&glob, name);
            }
            assert_eq!(
                cursor.accepts(&glob),
                glob.matches(&lit(path)?),
                "{pattern} against {path}"
            );
        }
    }
    let glob = matcher("/w/*/proj/*.pem")?;
    let inside = glob.cursor(&glob.static_base()?).child(&glob, "a");
    assert!(inside.child(&glob, "other").is_dead());
    assert!(inside.reaches_beneath(&glob));
    let proj = inside.child(&glob, "proj");
    assert!(!proj.is_dead());
    assert!(proj.child(&glob, "a").child(&glob, "b").is_dead());
    assert!(!proj.child(&glob, "a").child(&glob, "b").reaches_beneath(&glob));
    Ok(())
}

#[test]
fn a_cursor_at_the_root_starts_at_the_beginning_of_the_pattern() -> TestResult {
    let glob = matcher("/**/x")?;
    let at_root = glob.cursor(&lit("/")?);
    assert!(!at_root.is_dead());
    assert!(at_root.child(&glob, "x").accepts(&glob));
    Ok(())
}

#[test]
fn static_base_stops_at_first_wildcard_segment() -> TestResult {
    assert_eq!(
        matcher("/home/u/.config/*/token")?.static_base()?,
        lit("/home/u/.config")?
    );
    assert_eq!(matcher("/**/x")?.static_base()?, lit("/")?);
    assert_eq!(matcher("/work/a[b/c]d/**/*.key")?.static_base()?, lit("/work")?);
    assert_eq!(matcher("/work/ab*/x")?.static_base()?, lit("/work")?);
    assert_eq!(matcher("/*")?.static_base()?, lit("/")?);
    Ok(())
}

#[test]
fn a_pattern_stands_for_itself_with_each_wildcard_named_x() -> TestResult {
    let sample = |raw: &str| -> Result<LiteralPath, Box<dyn std::error::Error>> { Ok(matcher(raw)?.sample_path()?) };
    assert_eq!(sample("/a/*/b")?, lit("/a/x/b")?);
    assert_eq!(sample("/a/**/b")?, lit("/a/x/b")?);
    assert_eq!(sample("/a/f?[0-9].txt")?, lit("/a/fxx.txt")?);
    assert_eq!(sample("/**/.env")?, lit("/x/.env")?);
    assert_eq!(sample("/a/f?*.txt")?, lit("/a/fx.txt")?);
    Ok(())
}

#[test]
fn an_anchored_pattern_reads_its_anchor_as_a_name() -> TestResult {
    let pattern = GlobPattern::anchored(lit("/w/a[b]")?, "/*.pem")?;
    assert_eq!(pattern.as_str(), "/w/a[b]/*.pem");
    let matcher = GlobMatcher::new(&pattern);
    assert_eq!(matcher.static_base()?, lit("/w/a[b]")?);
    assert!(matcher.matches(&lit("/w/a[b]/k.pem")?));
    assert!(!matcher.matches(&lit("/w/ab/k.pem")?));
    let at_root = GlobMatcher::new(&GlobPattern::anchored(lit("/")?, "/*.pem")?);
    assert!(at_root.matches(&lit("/k.pem")?));
    Ok(())
}

#[test]
fn literal_ancestry() -> TestResult {
    let path = lit("/a/b/c")?;
    let chain: Vec<String> = path.ancestors().map(|p| p.as_str().to_string()).collect();
    assert_eq!(chain, ["/a/b/c", "/a/b", "/a", "/"]);
    assert_eq!(lit("/a")?.parent(), Some(lit("/")?));
    assert_eq!(lit("/")?.parent(), None);
    assert!(lit("/")?.is_root());
    assert!(!path.is_root());
    assert!(!lit("/ab")?.is_within(&lit("/a")?));
    assert!(path.is_within(&lit("/")?));
    assert!(path.is_within(&path));
    assert!(path.is_within(&lit("/a/b")?));
    assert_eq!(path.components().collect::<Vec<_>>(), ["a", "b", "c"]);
    Ok(())
}

#[test]
fn backslash_is_a_name_character() -> TestResult {
    let backslashed = lit(r"/w/a\b")?;
    assert_eq!(backslashed.as_str(), r"/w/a\b");
    assert_eq!(backslashed.components().collect::<Vec<_>>(), ["w", r"a\b"]);
    assert!(!lit("/w/a/b")?.is_within(&backslashed));
    assert!(!backslashed.is_within(&lit("/w/a")?));
    Ok(())
}

#[test]
fn only_wildcards_are_wildcards() {
    assert!(!t("a").is_wildcard());
    assert!(!SEP.is_wildcard());
    for token in [ANY, STAR, GS, GSS, set("a")] {
        assert!(token.is_wildcard());
    }
}
