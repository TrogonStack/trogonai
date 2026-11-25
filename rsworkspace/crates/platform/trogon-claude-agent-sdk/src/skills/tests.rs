use super::{effective_allowed_tools, effective_setting_sources, initialize_skills};
use crate::{ArgvStyle, ClaudeAgentOptions, SettingSource, Skills, ToolRule};

fn options(style: ArgvStyle, skills: Option<Skills>, allowed: &[&str]) -> ClaudeAgentOptions {
    ClaudeAgentOptions {
        argv_style: style,
        skills,
        allowed_tools: allowed.iter().map(|rule| ToolRule::from(*rule)).collect(),
        ..ClaudeAgentOptions::default()
    }
}

fn named(names: &[&str]) -> Option<Skills> {
    Some(Skills::Named(names.iter().map(|name| (*name).to_owned()).collect()))
}

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn all_appends_the_bare_skill_tool_once_in_both_dialects() -> TestResult {
    for style in [ArgvStyle::TypeScript, ArgvStyle::Python] {
        assert_eq!(
            effective_allowed_tools(&options(style, Some(Skills::All), &["Read"]))?,
            ["Read", "Skill"]
        );
        assert_eq!(
            effective_allowed_tools(&options(style, Some(Skills::All), &["Skill"]))?,
            ["Skill"]
        );
    }
    Ok(())
}

#[test]
fn named_skills_become_specifiers_and_repeats_differ_by_dialect() -> TestResult {
    let names = named(&["a", "b", "a", "c"]);
    assert_eq!(
        effective_allowed_tools(&options(ArgvStyle::TypeScript, names.clone(), &["Skill(b)"]))?,
        ["Skill(b)", "Skill(a)", "Skill(a)", "Skill(c)"]
    );
    assert_eq!(
        effective_allowed_tools(&options(ArgvStyle::Python, names, &["Skill(b)"]))?,
        ["Skill(b)", "Skill(a)", "Skill(c)"]
    );
    Ok(())
}

#[test]
fn no_skills_leaves_the_list_alone() -> TestResult {
    assert_eq!(
        effective_allowed_tools(&options(ArgvStyle::TypeScript, None, &["Read", "Read"]))?,
        ["Read", "Read"]
    );
    Ok(())
}

#[test]
fn python_defaults_the_setting_sources_only_when_skills_are_set() {
    let python = options(ArgvStyle::Python, Some(Skills::All), &[]);
    assert_eq!(
        effective_setting_sources(&python),
        Some(vec!["user".to_owned(), "project".to_owned()])
    );
    assert_eq!(effective_setting_sources(&options(ArgvStyle::Python, None, &[])), None);
    assert_eq!(
        effective_setting_sources(&options(ArgvStyle::TypeScript, Some(Skills::All), &[])),
        None
    );
    let explicit = ClaudeAgentOptions {
        setting_sources: Some(vec![SettingSource::Local]),
        ..python
    };
    assert_eq!(effective_setting_sources(&explicit), Some(vec!["local".to_owned()]));
    let empty = ClaudeAgentOptions {
        setting_sources: Some(Vec::new()),
        ..explicit
    };
    assert_eq!(effective_setting_sources(&empty), Some(Vec::new()));
}

#[test]
fn only_an_explicit_list_reaches_the_initialize_request() {
    assert!(initialize_skills(&options(ArgvStyle::TypeScript, Some(Skills::All), &[])).is_none());
    assert!(initialize_skills(&options(ArgvStyle::TypeScript, None, &[])).is_none());
    assert_eq!(
        initialize_skills(&options(ArgvStyle::Python, named(&["a"]), &[])),
        Some(vec![serde_json::Value::String("a".to_owned())])
    );
}

fn message(style: ArgvStyle, name: &str) -> String {
    effective_allowed_tools(&options(style, named(&[name]), &[]))
        .map_or_else(|error| error.to_string(), |_| String::new())
}

#[test]
fn typescript_names_are_rejected_with_the_upstream_text() {
    let cases = [
        ("", "Skill names must be non-empty strings."),
        ("   ", "Skill names must be non-empty strings."),
        (
            " a",
            "Invalid skill name \" a\": leading or trailing whitespace is not allowed \u{2014} the Skill tool trims the invoked name, so a padded rule can never match. Remove the padding.",
        ),
        (
            "a(b",
            "Invalid skill name \"a(b\": parentheses, commas, and control characters are not allowed in skill names. Skill names match the skill's directory name (or 'plugin:skill' for plugin-qualified skills); rename the skill if its directory name contains these characters.",
        ),
        ("*", "Invalid skill name '*': use skills: 'all' to enable every skill."),
        (
            "x:*",
            "Invalid skill name \"x:*\": wildcard-suffix names are not allowed; list each skill by its exact name.",
        ),
        (
            "/x",
            "Invalid skill name \"/x\": skill names may not start with '/'. Skills are invoked as slash commands, but the skills option takes the skill's canonical name \u{2014} the directory name, or 'plugin:skill'.",
        ),
        (
            "a\\\\b",
            "Invalid skill name \"a\\\\\\\\b\": consecutive backslashes are not allowed \u{2014} the permission-rule parser collapses escaped backslashes, so the rule would name a different skill. Rename the skill.",
        ),
        (
            "a\\",
            "Invalid skill name \"a\\\\\": names may not end with an unpaired backslash.",
        ),
    ];
    for (name, expected) in cases {
        assert_eq!(message(ArgvStyle::TypeScript, name), expected, "{name:?}");
    }
}

#[test]
fn python_names_are_rejected_with_the_upstream_text() {
    let cases = [
        ("", "Skill names must be non-empty strings"),
        (
            "a ",
            "Invalid skill name 'a ': leading or trailing whitespace can never match \u{2014} the Skill tool trims the invoked name.",
        ),
        (
            "a,b",
            "Invalid skill name 'a,b': parentheses, commas, control characters, and byte-order marks are not allowed. Names match the skill's directory name, or 'plugin:skill' for plugin-qualified skills.",
        ),
        ("*", "Invalid skill name '*': use skills=\"all\" to enable every skill."),
        (
            "x *",
            "Invalid skill name 'x *': wildcard-suffix names are not allowed; list each skill by its exact name.",
        ),
        (
            "/x",
            "Invalid skill name '/x': skill names may not start with '/'. The skills option takes the canonical name, not the slash-command form.",
        ),
        (
            "a\\\\b",
            "Invalid skill name 'a\\\\\\\\b': consecutive backslashes are not allowed \u{2014} the per-rule parser collapses them, so the rule would name a different skill.",
        ),
        (
            "a\\",
            "Invalid skill name 'a\\\\': names may not end with an unpaired backslash.",
        ),
    ];
    for (name, expected) in cases {
        assert_eq!(message(ArgvStyle::Python, name), expected, "{name:?}");
    }
}

#[test]
fn the_dialects_disagree_on_whitespace_and_the_byte_order_mark() {
    assert!(message(ArgvStyle::TypeScript, "a\u{feff}").contains("leading or trailing whitespace"));
    assert!(message(ArgvStyle::Python, "a\u{feff}").contains("byte-order marks"));
    assert!(message(ArgvStyle::TypeScript, "a\u{1c}").contains("control characters"));
    assert!(message(ArgvStyle::Python, "a\u{1c}").contains("leading or trailing whitespace"));
    assert!(message(ArgvStyle::TypeScript, "a\u{2028}").contains("leading or trailing whitespace"));
    assert!(message(ArgvStyle::TypeScript, "a\u{a0}").contains("leading or trailing whitespace"));
}
