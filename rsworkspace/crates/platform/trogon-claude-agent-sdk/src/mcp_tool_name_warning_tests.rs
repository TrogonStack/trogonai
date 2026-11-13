//! Replays `tests/fixtures/sdk_mcp/tool_name_warnings.json`: the lines the bundled TypeScript check prints for
//! each name, recorded by running it from the pinned `sdk.mjs`.

use super::*;
use serde::Deserialize;

const FIXTURE: &str = include_str!("../tests/fixtures/sdk_mcp/tool_name_warnings.json");

#[derive(Deserialize)]
struct Fixture {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    lines: Vec<String>,
}

#[test]
fn tool_name_warning_lines_match_the_typescript_console_output() -> Result<(), serde_json::Error> {
    let fixture: Fixture = serde_json::from_str(FIXTURE)?;
    assert!(!fixture.cases.is_empty());
    for case in fixture.cases {
        assert_eq!(tool_name_warning_lines(&case.name), case.lines, "{:?}", case.name);
    }
    Ok(())
}

#[test]
fn names_inside_the_protocol_limits_print_nothing() {
    for name in ["a", "ok_name", "Ok.Name-1"] {
        assert!(tool_name_warnings(name).is_empty(), "{name}");
        assert!(tool_name_warning_lines(name).is_empty(), "{name}");
    }
}

#[test]
fn each_half_of_a_surrogate_pair_is_listed_once_as_a_replacement_character() {
    assert_eq!(
        tool_name_warnings("\u{1F600}\u{1F603}"),
        [
            "Tool name contains invalid characters: \"\u{FFFD}\", \"\u{FFFD}\", \"\u{FFFD}\"",
            "Allowed characters are: A-Z, a-z, 0-9, underscore (_), dash (-), and dot (.)",
        ]
    );
}
