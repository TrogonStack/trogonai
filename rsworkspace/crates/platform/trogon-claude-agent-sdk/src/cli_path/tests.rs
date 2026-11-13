use super::is_windows_batch_cli;

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
    assert_eq!(super::python_repr("a\tb'\"c"), "'a\\tb\\'\"c'");
}

#[test]
fn batch_paths_run_unless_the_python_refusal_is_selected() {
    assert_eq!(super::WindowsBatchCli::default(), super::WindowsBatchCli::Run);
    assert!(super::reject_windows_batch_cli("claude.cmd".as_ref(), super::WindowsBatchCli::Run).is_ok());
}
