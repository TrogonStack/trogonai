use std::{collections::BTreeMap, ffi::OsString, path::PathBuf};

use super::{path_for, process_debug_file};
use crate::{
    environment_plan::Inherited,
    test_support::{FixedEnvironment, WithPasswd},
};

fn inherited(entries: &[(&str, &str)]) -> Inherited {
    entries
        .iter()
        .map(|(key, value)| (OsString::from(key), OsString::from(value)))
        .collect()
}

fn environment(home: Option<&str>, passwd: Option<&str>) -> WithPasswd<FixedEnvironment> {
    WithPasswd {
        environment: FixedEnvironment {
            directory: PathBuf::from("/work"),
            variables: home
                .map(|home| (OsString::from("HOME"), OsString::from(home)))
                .into_iter()
                .collect::<BTreeMap<_, _>>(),
        },
        effective: passwd.map(OsString::from),
        real: passwd.map(OsString::from),
    }
}

#[test]
fn nothing_is_derived_unless_the_variable_is_truthy() {
    let id = uuid::Uuid::nil();
    let environment = environment(Some("/h"), None);
    for value in ["", "0", "false", "no", "off", "2"] {
        assert_eq!(
            path_for(&environment, &inherited(&[("DEBUG_CLAUDE_AGENT_SDK", value)]), id),
            Ok(None)
        );
    }
    assert_eq!(path_for(&environment, &inherited(&[]), id), Ok(None));
}

#[test]
fn a_truthy_variable_selects_the_debug_directory_of_the_config_dir() {
    let id = uuid::Uuid::nil();
    let expected = PathBuf::from(format!("/h/.claude/debug/sdk-{id}.txt"));
    for value in ["1", "TRUE", " yes ", "On"] {
        assert_eq!(
            path_for(
                &environment(Some("/h"), None),
                &inherited(&[("DEBUG_CLAUDE_AGENT_SDK", value)]),
                id
            ),
            Ok(Some(expected.clone()))
        );
    }
    let explicit = inherited(&[("DEBUG_CLAUDE_AGENT_SDK", "1"), ("CLAUDE_CONFIG_DIR", "/c")]);
    assert_eq!(
        path_for(&environment(Some("/h"), None), &explicit, id),
        Ok(Some(PathBuf::from(format!("/c/debug/sdk-{id}.txt"))))
    );
}

#[test]
fn the_home_comes_from_the_process_environment_like_os_homedir() {
    let id = uuid::Uuid::nil();
    let debug = inherited(&[("DEBUG_CLAUDE_AGENT_SDK", "1"), ("HOME", "/inherited")]);
    assert_eq!(
        path_for(&environment(Some("/process"), None), &debug, id),
        Ok(Some(PathBuf::from(format!("/process/.claude/debug/sdk-{id}.txt"))))
    );
}

#[test]
fn an_unset_home_falls_back_to_the_passwd_entry_and_an_empty_one_is_used_as_is() {
    let id = uuid::Uuid::nil();
    let debug = inherited(&[("DEBUG_CLAUDE_AGENT_SDK", "1")]);
    assert_eq!(
        path_for(&environment(None, Some("/passwd")), &debug, id),
        Ok(Some(PathBuf::from(format!("/passwd/.claude/debug/sdk-{id}.txt"))))
    );
    assert_eq!(
        path_for(&environment(Some(""), Some("/passwd")), &debug, id),
        Ok(Some(PathBuf::from(format!(".claude/debug/sdk-{id}.txt"))))
    );
}

#[test]
fn a_messy_home_is_normalized_like_path_join() {
    let id = uuid::Uuid::nil();
    let debug = inherited(&[("DEBUG_CLAUDE_AGENT_SDK", "1")]);
    assert_eq!(
        path_for(&environment(Some("/h//a/../b/"), None), &debug, id),
        Ok(Some(PathBuf::from(format!("/h/b/.claude/debug/sdk-{id}.txt"))))
    );
}

#[test]
fn the_config_dir_is_normalized_to_nfc() {
    let decomposed = inherited(&[("DEBUG_CLAUDE_AGENT_SDK", "1"), ("CLAUDE_CONFIG_DIR", "/e\u{301}")]);
    let path = path_for(&environment(None, None), &decomposed, uuid::Uuid::nil())
        .unwrap_or_default()
        .unwrap_or_default();
    assert!(path.starts_with("/\u{e9}"));
}

#[test]
fn a_missing_home_fails_like_the_node_constructor() {
    let debug = inherited(&[("DEBUG_CLAUDE_AGENT_SDK", "1")]);
    let error = path_for(&environment(None, None), &debug, uuid::Uuid::nil()).expect_err("no home directory");
    assert_eq!(
        error.message(),
        "A system error occurred: uv_os_homedir returned ENOENT (no such file or directory)"
    );
    // The same lookup is not needed when the variable is not truthy.
    assert_eq!(
        path_for(&environment(None, None), &inherited(&[]), uuid::Uuid::nil()),
        Ok(None)
    );
}

#[test]
fn the_first_successful_call_decides_for_the_process() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let config = directory.path().join("config");
    let debug = inherited(&[
        ("DEBUG_CLAUDE_AGENT_SDK", "1"),
        ("CLAUDE_CONFIG_DIR", config.to_str().expect("utf-8 path")),
    ]);
    let environment = environment(None, None);
    let first = process_debug_file(&environment, &debug)
        .expect("derived")
        .expect("a path");
    assert!(first.starts_with(config.join("debug")));
    assert!(config.join("debug").is_dir());
    assert_eq!(process_debug_file(&environment, &inherited(&[])), Ok(Some(first)));
}
