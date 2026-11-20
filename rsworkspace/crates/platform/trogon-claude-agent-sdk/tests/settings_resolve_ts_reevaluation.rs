//! Repeated resolves of the managed settings. `resolveSettings` evaluates them for the settings, for their origin and
//! for whether the machine policy loaded, and Node runs out of stack at a depth that depends on what ran before. The
//! engine reads every managed document once and fails each one nested `SETTINGS_DEPTH_BOUND` levels or deeper, so the
//! answer to a resolve is the same on a fresh state, on a state that resolved before, and however often it repeats.

use trogon_claude_agent_sdk::{
    ResolveSettingsError, ResolveSettingsOptions, ResolverState, SETTINGS_DEPTH_BOUND, resolve_settings_with_state,
};

#[path = "support/fake_settings_host.rs"]
mod fake_settings_host;

use fake_settings_host::{FakeHost, exited, ok};

const MACHINE: &str = "/mnt/c/Windows/System32/reg.exe query HKLM\\SOFTWARE\\Policies\\ClaudeCode /v Settings";
const USER: &str = "/mnt/c/Windows/System32/reg.exe query HKCU\\SOFTWARE\\Policies\\ClaudeCode /v Settings";
const MISSING: &str = "ERROR: The system was unable to find the specified registry key or value.";
const INHERITS: &str = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Policies\\ClaudeCode\r\n    Settings    REG_SZ    {\"wslInheritsWindowsSettings\":true}\r\n";
const WINDOWS_DROP_INS: &str = "/mnt/c/Program Files/ClaudeCode/managed-settings.d";
const LINUX_MANAGED: &str = "/etc/claude-code/managed-settings.json";

/// Nested arrays under one key that make a document one level below the bound.
const WITHIN: usize = SETTINGS_DEPTH_BOUND - 2;

/// Nested arrays under one key that make a document as deep as the bound.
const AT_BOUND: usize = SETTINGS_DEPTH_BOUND - 1;

/// A WSL session whose machine policy only asks to inherit the Windows managed settings, whose Windows drop-in folder
/// cannot be listed for the next `unlistable` listings and is gone after them, and whose Linux managed settings file
/// holds `arrays` nested arrays under one key. An unreadable drop-in folder leaves the Windows side unloaded, which
/// keeps the evaluation there; a Windows side that holds nothing sends it to the Linux managed settings file.
fn host(arrays: usize, unlistable: usize) -> FakeHost {
    let linux = format!("{{\"x\":{}null{}}}", "[".repeat(arrays), "]".repeat(arrays));
    FakeHost::linux()
        .variable("WSL_DISTRO_NAME", "Ubuntu")
        .command(MACHINE, ok(INHERITS))
        .command(USER, exited(1, MISSING))
        .file(LINUX_MANAGED, &linux)
        .unlistable(WINDOWS_DROP_INS, unlistable)
}

fn resolve_on(arrays: usize, unlistable: usize, state: &ResolverState) -> Result<(), ResolveSettingsError> {
    let options: ResolveSettingsOptions = serde_json::from_str(r#"{"settingSources":[]}"#).expect("options");
    resolve_settings_with_state(&host(arrays, unlistable), &options, state).map(drop)
}

fn resolve(arrays: usize, unlistable: usize) -> Result<(), ResolveSettingsError> {
    resolve_on(arrays, unlistable, &ResolverState::new())
}

#[test]
fn a_windows_drop_in_folder_that_stays_unreadable_keeps_every_evaluation_off_the_linux_managed_file() {
    assert!(resolve(AT_BOUND, usize::MAX).is_ok());
}

#[test]
fn a_linux_managed_file_as_deep_as_the_bound_rejects_the_resolve() {
    assert!(matches!(
        resolve(AT_BOUND, 0),
        Err(ResolveSettingsError::MaximumCallStack)
    ));
}

#[test]
fn a_linux_managed_file_below_the_bound_resolves() {
    assert!(resolve(WITHIN, 0).is_ok());
}

#[test]
fn repeated_resolves_on_one_state_answer_as_resolves_on_a_fresh_state_do() {
    let state = ResolverState::new();
    for _ in 0..3 {
        assert!(matches!(
            resolve_on(AT_BOUND, 0, &state),
            Err(ResolveSettingsError::MaximumCallStack)
        ));
        assert!(resolve_on(WITHIN, 0, &state).is_ok());
        assert!(resolve_on(AT_BOUND, usize::MAX, &state).is_ok());
    }
}
