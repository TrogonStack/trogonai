#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        serde_json_macro,
        reason = "Tests exercise exact upstream JSON fixtures, including malformed and extension shapes."
    )
)]

use super::*;
use serde_json::Value;

const ERRORS: &str = include_str!("../tests/fixtures/sdk_mcp/regexp_errors.json");

#[test]
fn compile_errors_read_like_the_node_regexp_constructor() -> Result<(), Box<dyn std::error::Error>> {
    let fixture: Value = serde_json::from_str(ERRORS)?;
    let cases = fixture["cases"].as_array().ok_or("cases is not an array")?;
    assert!(cases.len() > 20);
    for case in cases {
        let source = case[0].as_str().ok_or("source is not a string")?;
        let flags = case[1].as_str().ok_or("flags is not a string")?;
        let actual = compile(source, flags).err();
        assert_eq!(actual.as_deref(), case[2].as_str(), "/{source}/{flags}");
    }
    Ok(())
}

#[test]
fn matches_follow_ecmascript_lookaround_backreferences_named_groups_and_unicode_flags() -> Result<(), String> {
    let check = |source: &str, flags: &str, text: &str| compile(source, flags).map(|regex| test(&regex, text, flags));
    assert_eq!(check("^(?=.*a)b", "", "ba"), Ok(true));
    assert_eq!(check("(?<!a)b", "", "ab"), Ok(false));
    assert_eq!(check("(a)\\1", "", "aa"), Ok(true));
    assert_eq!(check("(?<x>a)\\k<x>", "", "ab"), Ok(false));
    assert_eq!(check("^.$", "", "\u{1f600}"), Ok(false));
    assert_eq!(check("^.$", "u", "\u{1f600}"), Ok(true));
    assert_eq!(check("^\\p{L}$", "u", "e"), Ok(true));
    assert_eq!(check("^[\\p{L}--[a-z]]$", "v", "e"), Ok(false));
    assert_eq!(check("a", "i", "A"), Ok(true));
    Ok(())
}
