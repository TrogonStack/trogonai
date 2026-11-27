use std::error::Error;

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

fn completed(exit_code: i32, stdout: &str) -> RunCommandResponse {
    RunCommandResponse::Completed(CommandOutput {
        exit_code,
        stdout: stdout.as_bytes().to_vec(),
        ..CommandOutput::default()
    })
}

#[test]
fn only_the_missing_tool_exit_asks_for_a_diagnosis() {
    assert!(needs_diagnosis(&completed(MISSING_TOOL_EXIT, "")));
    assert!(!needs_diagnosis(&completed(0, "")));
    assert!(!needs_diagnosis(&completed(1, "")));
}

#[test]
fn known_tokens_are_reported_in_order() {
    let response = completed(
        0,
        "snapshot_tool_sh_present\nsnapshot_tool_nc_absent\nsnapshot_python3_socket_unavailable\n",
    );
    assert_eq!(
        diagnosis_checks(Some(&response)),
        vec![
            "snapshot_tool_sh_present",
            "snapshot_tool_nc_absent",
            "snapshot_python3_socket_unavailable"
        ]
    );
}

#[test]
fn unknown_output_never_reaches_the_report() {
    let response = completed(0, "secret-looking text\nsnapshot_tool_made_up_present\n");
    assert_eq!(diagnosis_checks(Some(&response)), vec![UNAVAILABLE]);
}

#[test]
fn a_missing_or_failed_reply_is_unavailable() {
    assert_eq!(diagnosis_checks(None), vec![UNAVAILABLE]);
}

#[test]
fn output_past_the_bound_is_not_read() {
    let mut text = "x".repeat(MAX_DIAGNOSTIC_BYTES);
    text.push_str("\nsnapshot_tool_sh_present\n");
    assert_eq!(diagnosis_checks(Some(&completed(0, &text))), vec![UNAVAILABLE]);
}

#[cfg(unix)]
use std::os::unix::fs::{PermissionsExt, symlink};
#[cfg(unix)]
use std::path::Path;
#[cfg(unix)]
use std::process::Command;

#[cfg(unix)]
fn fixture(tools: &[&str], python: Option<i32>) -> Result<tempfile::TempDir, Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    for tool in tools {
        symlink("/bin/sh", dir.path().join(tool))?;
    }
    if let Some(code) = python {
        let path = dir.path().join("python3");
        std::fs::write(&path, format!("#!/bin/sh\nexit {code}\n"))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(dir)
}

#[cfg(unix)]
fn run(script: &str, path: &Path) -> Result<(i32, String), Box<dyn Error>> {
    let output = Command::new("/bin/sh")
        .arg("-c")
        .arg(script)
        .env("PATH", path)
        .output()?;
    Ok((
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stdout).into_owned(),
    ))
}

#[cfg(unix)]
const ALL: &[&str] = &["sh", "timeout", "mkfifo", "base64", "node", "stty", "sleep", "cat"];

#[cfg(unix)]
#[test]
fn a_complete_image_with_nc_passes_the_probe() -> TestResult {
    let mut tools = ALL.to_vec();
    tools.push("nc");
    let dir = fixture(&tools, None)?;
    assert_eq!(run(REQUIRED_TOOLS_COMMAND, dir.path())?.0, 0);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_usable_python_socket_stands_in_for_nc() -> TestResult {
    let dir = fixture(ALL, Some(0))?;
    assert_eq!(run(REQUIRED_TOOLS_COMMAND, dir.path())?.0, 0);
    Ok(())
}

#[cfg(unix)]
#[test]
fn an_unusable_python_socket_fails_the_probe() -> TestResult {
    let dir = fixture(ALL, Some(1))?;
    assert_eq!(run(REQUIRED_TOOLS_COMMAND, dir.path())?.0, MISSING_TOOL_EXIT);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_missing_tool_is_named_by_the_diagnosis() -> TestResult {
    let dir = fixture(&["timeout", "cat"], None)?;
    assert_eq!(run(REQUIRED_TOOLS_COMMAND, dir.path())?.0, MISSING_TOOL_EXIT);
    let (_, stdout) = run(COMMAND, dir.path())?;
    let checks = diagnosis_checks(Some(&completed(0, &stdout)));
    assert!(checks.contains(&"snapshot_tool_mkfifo_absent".to_owned()));
    assert!(checks.contains(&"snapshot_tool_cat_present".to_owned()));
    assert!(checks.contains(&"snapshot_python3_socket_unavailable".to_owned()));
    Ok(())
}
