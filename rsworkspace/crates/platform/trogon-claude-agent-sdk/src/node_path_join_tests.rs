use serde_json::Value;

use super::{NoProcess, PathStyle, join};
use crate::node_path::NodeProcess;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const GOLDENS: &str = include_str!("../tests/fixtures/extract_from_bunfs/goldens.json");

#[test]
fn every_recorded_join_matches_node_for_both_path_styles() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let joins = goldens["joins"].as_array().ok_or("joins")?;
    assert!(joins.len() > 30);
    for case in joins {
        let parts: Vec<&str> = case["parts"]
            .as_array()
            .ok_or("parts")?
            .iter()
            .map(|part| part.as_str().ok_or("part"))
            .collect::<Result<_, _>>()?;
        assert_eq!(
            join(PathStyle::Posix, &parts),
            case["posix"].as_str().ok_or("posix")?,
            "posix {parts:?}"
        );
        assert_eq!(
            join(PathStyle::Win32, &parts),
            case["win32"].as_str().ok_or("win32")?,
            "win32 {parts:?}"
        );
    }
    Ok(())
}

#[test]
fn the_host_style_follows_the_target_platform() {
    let expected = if cfg!(windows) {
        PathStyle::Win32
    } else {
        PathStyle::Posix
    };
    assert_eq!(PathStyle::host(), expected);
}

#[test]
fn the_join_process_has_no_working_directory_or_environment() {
    assert_eq!(NoProcess.cwd(), Ok(Vec::new()));
    assert_eq!(NoProcess.env(&[0x48]), None);
}
