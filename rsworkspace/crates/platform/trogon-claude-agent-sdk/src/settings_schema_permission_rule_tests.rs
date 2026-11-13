use super::is_valid_permission_rule;

/// `(rule, valid in allow mode, valid in every other mode)`, each verdict taken from running the
/// pinned upstream validator.
const VERDICTS: &[(&str, bool, bool)] = &[
    ("", false, false),
    ("   ", false, false),
    ("\u{a0}\u{feff}", false, false),
    ("\u{85}", true, true),
    ("\u{180e}", true, true),
    ("Bash", true, true),
    ("bash", false, false),
    ("my_tool", true, true),
    ("_x", true, true),
    ("1abc", true, true),
    ("\u{df}", false, false),
    ("\u{1c5}", false, false),
    ("\u{10428}x", true, true),
    ("\u{10400}", true, true),
    ("\u{e9}", false, false),
    ("Task", true, true),
    ("task", false, false),
    ("(x)", true, true),
    ("()", false, false),
    ("Bash()", false, false),
    ("Bash(ls", false, false),
    ("Bash)", false, false),
    ("Bash(ls)x", false, false),
    ("Bash(a(b)c)", true, true),
    ("Ba(sh)(x)", true, true),
    ("Bash(ls\\)", false, false),
    ("Bash(\\(x)", true, true),
    ("Bash(a\\(b(c)", false, false),
    ("Bash(a\\)b(c)", false, false),
    ("Bash(\\\\(x)", true, true),
    ("Bash(\\\\\\(x)", true, true),
    ("Read(C:\\Projects\\(drafts)\\**)", false, false),
    ("Read(C:\\Projects\\\\(drafts)\\**)", true, true),
    ("Read(C:\\Projects\\[(]drafts)\\**)", true, true),
    ("mcp__", true, true),
    ("mcp__srv", true, true),
    ("mcp__srv__a__b", true, true),
    ("mcp__*", false, true),
    ("mcp__*__x", false, true),
    ("mcp__srv*", false, true),
    ("mcp__s*__x", false, true),
    ("mcp__srv__*", true, true),
    ("mcp__srv__get_*", true, true),
    ("mcp__srv(x)", false, false),
    ("mcp__srv(*)", false, false),
    ("mcp__srv()", false, false),
    ("*", false, true),
    ("Bash*", false, true),
    ("b*", false, false),
    ("a_*", false, true),
    ("Bash\\*", false, true),
    ("Read(a:*)", false, false),
    ("Read(a\\:*)", false, false),
    ("Read(*)", true, true),
    ("Read(:*)", false, false),
    ("Write(a:*)", false, false),
    ("Grep(a:*)", true, true),
    ("LS(a:*)", true, true),
    ("Cd(a:*)", false, false),
    ("WebSearch(a*)", false, false),
    ("WebSearch(a?)", false, false),
    ("WebSearch(*)", true, true),
    ("WebSearch(claude ai)", true, true),
    ("WebFetch(https://a)", false, false),
    ("WebFetch(http)", false, false),
    ("WebFetch(a.com)", false, false),
    ("WebFetch(domain:a.com)", true, true),
    ("WebFetch(domain:*.a.com)", true, true),
    ("WebFetch(Domain:a)", false, false),
    ("Bash(:*)", false, false),
    ("Bash( :*)", true, true),
    ("Bash(npm:*)", true, true),
    ("Bash(FOO=* ls)", true, true),
    ("Bash(git * status)", true, true),
    ("Bash(a*:*)", true, true),
    ("bash(ls)", false, false),
    ("Foo(x)", true, true),
    ("foo(x)", false, false),
    ("foo_bar(x)", true, true),
    ("Task(a:*)", true, true),
    ("Foo(a)b", false, false),
    ("Foo(\u{a0})", true, true),
    ("\u{1f600}(x)", true, true),
    ("mcp__\u{1f600}__*", true, true),
    ("Bash(\u{1f600}:*)", true, true),
];

#[test]
fn verdicts_match_the_upstream_validator() {
    let mut mismatches = Vec::new();
    for (rule, allow, other) in VERDICTS {
        if is_valid_permission_rule(rule, true) != *allow {
            mismatches.push(format!("allow {rule:?}: expected {allow}"));
        }
        if is_valid_permission_rule(rule, false) != *other {
            mismatches.push(format!("other {rule:?}: expected {other}"));
        }
    }
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

#[test]
fn only_allow_mode_refuses_wildcard_tool_names() {
    assert!(!is_valid_permission_rule("mcp__srv*", true));
    assert!(is_valid_permission_rule("mcp__srv*", false));
    assert!(is_valid_permission_rule("mcp__srv__*", true));
}

#[test]
fn a_surrogate_pair_first_unit_has_no_case() {
    assert!(is_valid_permission_rule("\u{10428}x", false));
    assert!(!is_valid_permission_rule("\u{df}x", false));
}
