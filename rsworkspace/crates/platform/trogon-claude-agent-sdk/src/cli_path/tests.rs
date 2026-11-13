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
