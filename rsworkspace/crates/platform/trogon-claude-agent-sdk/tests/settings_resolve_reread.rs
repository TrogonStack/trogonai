#![allow(clippy::expect_used, clippy::unwrap_used)]

//! The machine policy re-read throttle of `resolveSettings`: a long-lived process reads the policy again only when
//! the interval for the last reads has passed. The expectations of the first three tests come from running the
//! pinned TypeScript resolver on the same sequences with a fake clock (see the `mdm_reread_*` fixtures of the
//! differential test); the clock is the host's, so no test waits.

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{ResolveSettingsOptions, ResolverState, resolve_settings_with_state};

#[path = "support/fake_settings_host.rs"]
mod fake_settings_host;

use fake_settings_host::{FakeHost, exited, ok};

const MACHINE: &str = "/mnt/c/Windows/System32/reg.exe query HKLM\\SOFTWARE\\Policies\\ClaudeCode /v Settings";
const USER: &str = "/mnt/c/Windows/System32/reg.exe query HKCU\\SOFTWARE\\Policies\\ClaudeCode /v Settings";
const MISSING: &str = "ERROR: The system was unable to find the specified registry key or value.";
const MINUTE: u64 = 60_000;
const HALF_HOUR: u64 = 1_800_000;

fn registry_value(model: &str) -> String {
    format!(
        "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Policies\\ClaudeCode\r\n    Settings    REG_SZ    {{\"wslInheritsWindowsSettings\":true,\"model\":\"{model}\"}}\r\n"
    )
}

fn host() -> FakeHost {
    FakeHost::linux()
        .variable("WSL_DISTRO_NAME", "Ubuntu")
        .command(USER, exited(1, MISSING))
}

fn answers(host: &mut FakeHost, model: &str) {
    host.commands.insert(MACHINE.to_owned(), ok(&registry_value(model)));
}

fn answers_nothing(host: &mut FakeHost) {
    host.commands.insert(MACHINE.to_owned(), exited(1, MISSING));
}

fn denied(host: &mut FakeHost) {
    host.commands
        .insert(MACHINE.to_owned(), exited(1, "ERROR: Access is denied."));
}

fn resolve(host: &FakeHost, state: &ResolverState, at: u64) -> Value {
    host.set_time(at);
    let options: ResolveSettingsOptions = serde_json::from_value(json!({"settingSources": []})).expect("options");
    let resolved = resolve_settings_with_state(host, &options, state).expect("resolve");
    serde_json::to_value(resolved).expect("encode")
}

fn model(result: &Value) -> Option<&str> {
    result["effective"]["model"].as_str()
}

#[test]
fn an_answered_read_is_kept_for_half_an_hour() {
    let mut host = host();
    let state = ResolverState::new();
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &state, 0)), Some("A"));
    answers(&mut host, "B");
    assert_eq!(model(&resolve(&host, &state, 1_000)), Some("A"));
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR - 1)), Some("A"));
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR)), Some("B"));
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 1)), Some("B"));
}

#[test]
fn an_unanswered_read_is_retried_at_once_then_every_minute_and_the_answer_shows_on_the_call_after() {
    let mut host = host();
    let state = ResolverState::new();
    denied(&mut host);
    assert_eq!(model(&resolve(&host, &state, 0)), None);
    assert_eq!(host.ran().iter().filter(|line| *line == MACHINE).count(), 2);
    answers(&mut host, "B");
    assert_eq!(model(&resolve(&host, &state, MINUTE - 1)), None);
    assert_eq!(host.ran().iter().filter(|line| *line == MACHINE).count(), 2);
    assert_eq!(model(&resolve(&host, &state, MINUTE)), None);
    assert_eq!(host.ran().iter().filter(|line| *line == MACHINE).count(), 3);
    assert_eq!(model(&resolve(&host, &state, MINUTE + 1)), Some("B"));
}

#[test]
fn losing_all_policy_is_seen_twice_thirty_seconds_apart_before_it_is_believed() {
    let mut host = host();
    let state = ResolverState::new();
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &state, 0)), Some("A"));
    answers_nothing(&mut host);
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR)), Some("A"));
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 29_999)), Some("A"));
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 30_000)), None);
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 30_001)), None);
}

#[test]
fn a_read_that_takes_a_second_only_shows_on_the_next_call() {
    let mut host = host();
    let state = ResolverState::new();
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &state, 0)), Some("A"));
    denied(&mut host);
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR)), Some("A"));
    let mut slow = ok(&registry_value("B"));
    slow.duration_ms = 1_000;
    host.commands.insert(MACHINE.to_owned(), slow);
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 1)), Some("A"));
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 2_000)), Some("B"));
}

#[test]
fn a_read_under_a_second_is_waited_for() {
    let mut host = host();
    let state = ResolverState::new();
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &state, 0)), Some("A"));
    denied(&mut host);
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR)), Some("A"));
    let mut quick = ok(&registry_value("B"));
    quick.duration_ms = 999;
    host.commands.insert(MACHINE.to_owned(), quick);
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 1)), Some("B"));
}

#[test]
fn a_fresh_state_reads_the_host_on_every_call() {
    let mut host = host();
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &ResolverState::new(), 0)), Some("A"));
    answers(&mut host, "B");
    assert_eq!(model(&resolve(&host, &ResolverState::new(), 1)), Some("B"));
}
