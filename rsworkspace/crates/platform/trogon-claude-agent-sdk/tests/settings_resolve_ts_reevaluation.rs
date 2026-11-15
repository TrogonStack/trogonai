#![allow(clippy::expect_used, clippy::unwrap_used)]

//! The evaluations of the managed settings `resolveSettings` runs for their origin and for whether the machine policy
//! loaded, after it merged the sources. Each one lists the managed folders again, so a folder that changed since the
//! first evaluation can lead it to a managed settings file the first evaluation never read, and a file the file cache
//! runs out of stack copying rejects the resolve there as it does in the first evaluation.

use trogon_claude_agent_sdk::{
    NodeStack, ResolveSettingsError, ResolveSettingsOptions, ResolverState, resolve_settings_with_state,
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

/// Nested arrays under one key: past what the file cache copies in every evaluation of the managed settings on the
/// Node stack `NodeStack::NODE_24_DARWIN_ARM64` describes, and within what each of them reads.
const LINUX_ARRAYS: usize = 2000;

/// A WSL session whose machine policy only asks to inherit the Windows managed settings, whose Windows drop-in folder
/// cannot be listed for the next `unlistable` listings and is gone after them, and whose Linux managed settings file
/// is too deep for the file cache to copy. A resolve lists the Windows drop-in folder once to reserve the texts of the
/// managed documents and once in each evaluation of the managed settings. An unreadable drop-in folder leaves the
/// Windows side unloaded, which keeps the evaluation there; a Windows side that holds nothing sends it to the Linux
/// managed settings file.
fn host(unlistable: usize) -> FakeHost {
    let linux = format!("{{\"x\":{}null{}}}", "[".repeat(LINUX_ARRAYS), "]".repeat(LINUX_ARRAYS));
    FakeHost::linux()
        .variable("WSL_DISTRO_NAME", "Ubuntu")
        .command(MACHINE, ok(INHERITS))
        .command(USER, exited(1, MISSING))
        .file(LINUX_MANAGED, &linux)
        .unlistable(WINDOWS_DROP_INS, unlistable)
}

fn resolve(unlistable: usize) -> Result<(), ResolveSettingsError> {
    let options: ResolveSettingsOptions = serde_json::from_str(r#"{"settingSources":[]}"#).expect("options");
    let state = ResolverState::new().with_node_stack(NodeStack::NODE_24_DARWIN_ARM64);
    resolve_settings_with_state(&host(unlistable), &options, &state).map(drop)
}

#[test]
fn a_windows_drop_in_folder_that_stays_unreadable_keeps_every_evaluation_off_the_linux_managed_file() {
    assert!(resolve(usize::MAX).is_ok());
}

#[test]
fn the_first_evaluation_rejects_a_linux_managed_file_the_file_cache_runs_out_of_stack_copying() {
    assert!(matches!(resolve(0), Err(ResolveSettingsError::MaximumCallStack)));
}

#[test]
fn the_origin_evaluation_rejects_a_linux_managed_file_the_first_evaluation_never_read() {
    assert!(matches!(resolve(2), Err(ResolveSettingsError::MaximumCallStack)));
}

#[test]
fn the_machine_policy_evaluation_rejects_a_linux_managed_file_the_earlier_evaluations_never_read() {
    assert!(matches!(resolve(3), Err(ResolveSettingsError::MaximumCallStack)));
}
