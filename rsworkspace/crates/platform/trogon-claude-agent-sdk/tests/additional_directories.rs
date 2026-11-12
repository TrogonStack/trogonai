use std::{ffi::OsStr, path::PathBuf};
use trogon_claude_agent_sdk::ClaudeAgentOptions;

#[test]
fn additional_directories_preserve_order_duplicates_and_unmodified_paths() {
    assert!(ClaudeAgentOptions::default().additional_directories.is_empty());
    let paths = [
        "",
        "relative/folder",
        "folder with spaces",
        "unicode-café",
        "relative/folder",
    ];
    let options = ClaudeAgentOptions {
        additional_directories: paths.iter().map(PathBuf::from).collect(),
        ..ClaudeAgentOptions::default()
    };
    for (actual, expected) in options.additional_directories.iter().zip(paths) {
        assert_eq!(actual.as_os_str(), OsStr::new(expected));
    }
    assert_eq!(options.additional_directories.len(), paths.len());
    assert_eq!(options.clone().additional_directories, options.additional_directories);
}

#[test]
fn options_debug_shows_additional_directories() {
    let options = ClaudeAgentOptions {
        additional_directories: vec![PathBuf::from("fixture-additional-directory")],
        ..ClaudeAgentOptions::default()
    };
    let debug = format!("{options:?}");
    assert!(
        debug.contains(r#"additional_directories: ["fixture-additional-directory"]"#),
        "{debug}"
    );
}
