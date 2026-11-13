use std::ffi::OsString;

use super::path_for;
use crate::environment_plan::Inherited;

fn inherited(entries: &[(&str, &str)]) -> Inherited {
    entries
        .iter()
        .map(|(key, value)| (OsString::from(key), OsString::from(value)))
        .collect()
}

#[test]
fn nothing_is_derived_unless_the_variable_is_truthy() {
    let id = uuid::Uuid::nil();
    for value in ["", "0", "false", "no", "off", "2"] {
        assert!(path_for(&inherited(&[("DEBUG_CLAUDE_AGENT_SDK", value), ("HOME", "/h")]), id).is_none());
    }
    assert!(path_for(&inherited(&[("HOME", "/h")]), id).is_none());
}

#[test]
fn a_truthy_variable_selects_the_debug_directory_of_the_config_dir() {
    let id = uuid::Uuid::nil();
    let expected = format!("sdk-{id}.txt");
    for value in ["1", "TRUE", " yes ", "On"] {
        let home = inherited(&[("DEBUG_CLAUDE_AGENT_SDK", value), ("HOME", "/h"), ("USERPROFILE", "/h")]);
        let path = path_for(&home, id);
        assert_eq!(
            path.as_deref(),
            Some(std::path::Path::new("/h/.claude/debug").join(&expected).as_path())
        );
    }
    let explicit = inherited(&[
        ("DEBUG_CLAUDE_AGENT_SDK", "1"),
        ("CLAUDE_CONFIG_DIR", "/c"),
        ("HOME", "/h"),
    ]);
    assert_eq!(
        path_for(&explicit, id).as_deref(),
        Some(std::path::Path::new("/c/debug").join(&expected).as_path())
    );
}

#[test]
fn the_config_dir_is_normalized_to_nfc() {
    let decomposed = inherited(&[("DEBUG_CLAUDE_AGENT_SDK", "1"), ("CLAUDE_CONFIG_DIR", "/e\u{301}")]);
    let path = path_for(&decomposed, uuid::Uuid::nil()).unwrap_or_default();
    assert!(path.starts_with("/\u{e9}"));
}

#[test]
fn a_missing_home_derives_nothing() {
    assert!(path_for(&inherited(&[("DEBUG_CLAUDE_AGENT_SDK", "1")]), uuid::Uuid::nil()).is_none());
}
