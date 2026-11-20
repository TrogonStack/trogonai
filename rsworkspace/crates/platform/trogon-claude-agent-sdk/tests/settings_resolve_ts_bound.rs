//! The bound of the TypeScript dialect: every settings document nested `SETTINGS_DEPTH_BOUND` levels or deeper fails
//! the whole resolve with `MaximumCallStack`, and one a level shallower resolves, whatever host runs the resolve and
//! whatever resolved before it. Node's own failure depth depends on the compile state of V8, so the SDK fixes one
//! depth instead. Flag settings take no input in this crate (they are the `--settings` argument of the command line),
//! so no document of that source can be as deep as the bound here.

use std::thread;

use serde::Deserialize;
use trogon_claude_agent_sdk::{
    ResolveSettingsError, ResolveSettingsOptions, ResolverState, SETTINGS_DEPTH_BOUND, resolve_settings_json_with_state,
};

#[path = "support/fake_settings_host.rs"]
mod fake_settings_host;

use fake_settings_host::{FakeHost, exited, ok};

type Failure = Box<dyn std::error::Error + Send + Sync>;
type TestResult = Result<(), Failure>;

/// Room for walking, printing and dropping the deepest settings documents of the tests.
const DEEP_STACK_BYTES: usize = 256 * 1024 * 1024;

const USER_SETTINGS: &str = "/home/fixture/.claude/settings.json";
const DROP_INS: &str = "/etc/claude-code/managed-settings.d";
const DROP_IN: &str = "/etc/claude-code/managed-settings.d/10-deep.json";
const MACHINE: &str = "/mnt/c/Windows/System32/reg.exe query HKLM\\SOFTWARE\\Policies\\ClaudeCode /v Settings";
const USER: &str = "/mnt/c/Windows/System32/reg.exe query HKCU\\SOFTWARE\\Policies\\ClaudeCode /v Settings";
const MISSING: &str = "ERROR: The system was unable to find the specified registry key or value.";

const BELOW: usize = SETTINGS_DEPTH_BOUND - 1;

/// A document of `depth` levels: objects under `x`, around the string `"deep"`.
fn document(depth: usize) -> String {
    format!("{}\"deep\"{}", "{\"x\":".repeat(depth), "}".repeat(depth))
}

fn linux_user(depth: usize) -> FakeHost {
    FakeHost::linux().file(USER_SETTINGS, &document(depth))
}

fn linux_drop_in(depth: usize) -> FakeHost {
    FakeHost::linux()
        .directory(DROP_INS, &["10-deep.json"])
        .file(DROP_IN, &document(depth))
}

/// A WSL session whose machine policy is the registry text `settings`.
fn registry(settings: &str) -> FakeHost {
    let answer =
        format!("\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Policies\\ClaudeCode\r\n    Settings    REG_SZ    {settings}\r\n");
    FakeHost::linux()
        .variable("WSL_DISTRO_NAME", "Ubuntu")
        .command(MACHINE, ok(&answer))
        .command(USER, exited(1, MISSING))
}

fn options(json: &str) -> Result<ResolveSettingsOptions, Failure> {
    let mut deserializer = serde_json::Deserializer::from_str(json);
    deserializer.disable_recursion_limit();
    Ok(ResolveSettingsOptions::deserialize(&mut deserializer)?)
}

fn resolve(
    host: &FakeHost,
    options: &ResolveSettingsOptions,
    state: &ResolverState,
) -> Result<String, ResolveSettingsError> {
    resolve_settings_json_with_state(host, options, state)
}

/// Runs `check` on a thread with room for documents this deep.
fn deep(check: fn() -> TestResult) -> TestResult {
    thread::Builder::new()
        .stack_size(DEEP_STACK_BYTES)
        .spawn(check)?
        .join()
        .map_err(|_| "the deep settings worker panicked")?
}

fn rejected(answer: &Result<String, ResolveSettingsError>) -> bool {
    matches!(answer, Err(ResolveSettingsError::MaximumCallStack))
}

/// The answers for a host that resolves below the bound and one that does not, on a fresh state and on one that
/// resolved before, which are the same.
fn bounded(below: &FakeHost, at: &FakeHost, options: &ResolveSettingsOptions) -> Result<String, Failure> {
    let fresh = resolve(below, options, &ResolverState::new())?;
    let shared = ResolverState::new();
    assert!(rejected(&resolve(at, options, &shared)));
    assert_eq!(resolve(below, options, &shared)?, fresh);
    assert!(rejected(&resolve(at, options, &shared)));
    assert!(rejected(&resolve(at, options, &ResolverState::new())));
    Ok(fresh)
}

fn effective_of(result: &str) -> Option<&str> {
    let rest = result.strip_prefix("{\"effective\":")?;
    rest.find(",\"provenance\":").map(|end| &rest[..end])
}

#[test]
fn a_user_settings_document_below_the_bound_reaches_effective_and_one_at_it_fails() -> TestResult {
    deep(|| {
        let plain = options("{}")?;
        let result = bounded(&linux_user(BELOW), &linux_user(SETTINGS_DEPTH_BOUND), &plain)?;
        assert_eq!(effective_of(&result), Some(document(BELOW).as_str()));
        Ok(())
    })
}

#[test]
fn a_managed_drop_in_below_the_bound_reaches_effective_and_one_at_it_fails() -> TestResult {
    deep(|| {
        let plain = options("{}")?;
        let result = bounded(&linux_drop_in(BELOW), &linux_drop_in(SETTINGS_DEPTH_BOUND), &plain)?;
        assert_eq!(effective_of(&result), Some(document(BELOW).as_str()));
        Ok(())
    })
}

/// A state keeps the machine policy it read until it reads it again, as upstream keeps it for the process, so each
/// machine resolves on states of its own: one state never reads two machines.
#[test]
fn a_registry_policy_below_the_bound_resolves_and_one_at_it_fails() -> TestResult {
    deep(|| {
        let plain = options("{}")?;
        let below = registry(&document(BELOW));
        let at = registry(&document(SETTINGS_DEPTH_BOUND));
        let fresh = resolve(&below, &plain, &ResolverState::new())?;
        let held = ResolverState::new();
        assert_eq!(resolve(&below, &plain, &held)?, fresh);
        assert_eq!(resolve(&below, &plain, &held)?, fresh);
        let rejecting = ResolverState::new();
        assert!(rejected(&resolve(&at, &plain, &rejecting)));
        assert!(rejected(&resolve(&at, &plain, &rejecting)));
        assert!(rejected(&resolve(&at, &plain, &ResolverState::new())));
        Ok(())
    })
}

#[test]
fn the_settings_passed_in_the_options_below_the_bound_resolve_and_ones_at_it_fail() -> TestResult {
    deep(|| {
        let host = FakeHost::linux();
        for member in ["managedSettings", "serverManagedSettings"] {
            let below = options(&format!("{{\"{member}\":{}}}", document(BELOW)))?;
            let at = options(&format!("{{\"{member}\":{}}}", document(SETTINGS_DEPTH_BOUND)))?;
            let fresh = resolve(&host, &below, &ResolverState::new())?;
            let shared = ResolverState::new();
            assert!(rejected(&resolve(&host, &at, &shared)));
            assert_eq!(resolve(&host, &below, &shared)?, fresh);
            assert!(rejected(&resolve(&host, &at, &ResolverState::new())));
        }
        Ok(())
    })
}

#[test]
fn a_document_of_arrays_at_the_bound_is_not_a_settings_object_and_is_dropped() -> TestResult {
    deep(|| {
        let nested = format!(
            "{}{}",
            "[".repeat(SETTINGS_DEPTH_BOUND),
            "]".repeat(SETTINGS_DEPTH_BOUND)
        );
        let host = FakeHost::linux().file(USER_SETTINGS, &nested);
        resolve(&host, &options("{}")?, &ResolverState::new())?;
        Ok(())
    })
}
