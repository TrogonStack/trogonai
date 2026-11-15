#[cfg(unix)]
use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

use super::{WindowsBatchCli, is_windows_batch_cli};

/// Exercise Windows lexical hazards on every host, without spawning a shell.
#[test]
fn windows_batch_paths_cover_components_streams_and_normalization() {
    for path in [
        "claude.bat",
        "CLAUDE.CMD",
        ".cmd",
        "C:claude.cmd",
        "claude:evil.cmd",
        "claude.cmd:stream",
        "claude.cmd. :stream",
        "claude.cmd   ...",
        "dir/claude.bat/../claude.exe",
        "claude.cmd\\...\\..",
        "dir//claude.cmd/./",
        "C:\\tools\\claude.BAT:stream.exe",
    ] {
        assert!(is_windows_batch_cli(path.as_ref()), "{path}");
    }
    for path in [
        "",
        "claude",
        "claude.exe",
        "C:\\tools\\claude.com",
        "dir.cmdx/claude.exe",
    ] {
        assert!(!is_windows_batch_cli(path.as_ref()), "{path}");
    }
}

#[test]
fn the_refusal_quotes_the_path_like_python_repr() {
    let error = super::CliPathError::WindowsBatchScript {
        path: "C:\\tools\\claude's.cmd".into(),
    };
    assert_eq!(
        error.to_string(),
        "Refusing to execute batch script \"C:\\\\tools\\\\claude's.cmd\": Windows runs .bat/.cmd files via cmd.exe, which can execute commands injected through CLI arguments, and no reliable escaping for cmd.exe exists. Use a native claude executable instead: install Claude Code natively (irm https://claude.ai/install.ps1 | iex), point ClaudeAgentOptions(cli_path=...) at a claude.exe, or install the claude-agent-sdk wheel for a platform that bundles claude.exe (e.g. Windows x64)."
    );
}

#[test]
fn batch_paths_run_unless_the_python_refusal_is_selected() {
    assert_eq!(WindowsBatchCli::default(), WindowsBatchCli::Run);
    assert!(super::reject_windows_batch_cli("claude.cmd".as_ref(), WindowsBatchCli::Run).is_ok());
}

/// Replays the refusals captured from Python's `_reject_windows_batch_cli` with each `platform.system()` answer.
#[cfg(unix)]
#[test]
fn the_python_refusal_matches_the_captured_goldens() -> Result<(), Box<dyn std::error::Error>> {
    let goldens: serde_json::Value =
        serde_json::from_str(include_str!("../../tests/fixtures/python_text/goldens.json"))?;
    let cases = goldens["windows_batch_cli"]
        .as_array()
        .ok_or("no windows_batch_cli goldens")?;
    assert_eq!(cases.len(), 28);
    for case in cases {
        let bytes = hex_bytes(case["path"].as_str().ok_or("no path")?)?;
        let path = OsStr::from_bytes(&bytes);
        let refusal = |windows| {
            super::refuse_batch_cli(path, WindowsBatchCli::Refuse, windows)
                .err()
                .map(|error| error.to_string())
        };
        assert_eq!(refusal(true).as_deref(), case["Windows"].as_str(), "{case}");
        assert_eq!(refusal(false).as_deref(), case["Linux"].as_str(), "{case}");
        assert_eq!(refusal(false).as_deref(), case["Darwin"].as_str(), "{case}");
        assert!(
            super::refuse_batch_cli(path, WindowsBatchCli::Run, true).is_ok(),
            "{case}"
        );
    }
    Ok(())
}

#[cfg(unix)]
fn hex_bytes(hex: &str) -> Result<Vec<u8>, std::num::ParseIntError> {
    (0..hex.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(hex.get(index..index + 2).unwrap_or_default(), 16))
        .collect()
}
