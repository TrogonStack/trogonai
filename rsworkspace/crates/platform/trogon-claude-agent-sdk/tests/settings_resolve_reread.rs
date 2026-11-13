#![allow(clippy::expect_used, clippy::unwrap_used)]

//! The machine policy re-read throttle of `resolveSettings`: a long-lived process reads the policy again only when
//! the interval for the last reads has passed, and a read upstream leaves running in the background counts from the
//! instant it ends. The expectations come from running the pinned TypeScript resolver on the same sequences with a
//! fake clock (see the `mdm_reread_*` fixtures of the differential test). The clock is the host's and a command
//! takes the `duration_ms` it reports, so only a test of a read held on its own thread waits, for at most a second
//! of steady time per call.

use std::{thread, time::Duration};

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{PolicyRereads, ResolveSettingsOptions, ResolverState, resolve_settings_with_state};

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

fn answers_after(host: &mut FakeHost, model: &str, milliseconds: u128) {
    let mut slow = ok(&registry_value(model));
    slow.duration_ms = milliseconds;
    host.commands.insert(MACHINE.to_owned(), slow);
}

fn machine_reads(host: &FakeHost) -> usize {
    host.ran().iter().filter(|line| *line == MACHINE).count()
}

/// Re-reads run on the calling thread, so whether one has ended by a call depends only on the host's clock.
fn inline_state() -> ResolverState {
    ResolverState::with_rereads(PolicyRereads::Inline)
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
    let state = inline_state();
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
    let state = inline_state();
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
    let state = inline_state();
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
    let state = inline_state();
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &state, 0)), Some("A"));
    denied(&mut host);
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR)), Some("A"));
    answers_after(&mut host, "B", 1_000);
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 1)), Some("A"));
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 1_001)), Some("B"));
}

#[test]
fn a_read_under_a_second_is_waited_for() {
    let mut host = host();
    let state = inline_state();
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &state, 0)), Some("A"));
    denied(&mut host);
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR)), Some("A"));
    answers_after(&mut host, "B", 999);
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 1)), Some("B"));
}

#[test]
fn a_call_behind_the_end_of_a_waited_read_reads_again_at_once() {
    let mut host = host();
    let state = inline_state();
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &state, 0)), Some("A"));
    answers_after(&mut host, "B", 600);
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR)), Some("B"));
    answers(&mut host, "C");
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 1)), Some("C"));
    answers(&mut host, "D");
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 2)), Some("C"));
    assert_eq!(machine_reads(&host), 3);
}

#[test]
fn a_clock_that_goes_back_from_where_the_last_read_ended_reads_again_at_once() {
    let mut host = host();
    let state = inline_state();
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR)), Some("A"));
    answers(&mut host, "B");
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR - 1)), Some("B"));
    answers(&mut host, "C");
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR)), Some("B"));
}

#[test]
fn a_call_while_a_read_runs_starts_no_other_and_goes_on_with_the_reading_in_force() {
    let mut host = host();
    let state = inline_state();
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &state, 0)), Some("A"));
    answers_after(&mut host, "B", 5_000);
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR)), Some("A"));
    assert_eq!(machine_reads(&host), 2);
    answers(&mut host, "C");
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 2_000)), Some("A"));
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 4_999)), Some("A"));
    assert_eq!(machine_reads(&host), 2);
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 5_000)), Some("B"));
    assert_eq!(machine_reads(&host), 2);
}

#[test]
fn a_retried_read_is_not_waited_for_however_quick() {
    let mut host = host();
    let state = inline_state();
    denied(&mut host);
    assert_eq!(model(&resolve(&host, &state, 0)), None);
    answers_after(&mut host, "B", 10);
    assert_eq!(model(&resolve(&host, &state, MINUTE)), None);
    assert_eq!(model(&resolve(&host, &state, MINUTE + 9)), None);
    assert_eq!(model(&resolve(&host, &state, MINUTE + 10)), Some("B"));
}

#[test]
fn both_registry_keys_are_read_at_once_so_the_slower_one_sets_how_long_the_read_takes() {
    let mut host = host();
    let state = inline_state();
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &state, 0)), Some("A"));
    answers_after(&mut host, "B", 600);
    let mut user = exited(1, MISSING);
    user.duration_ms = 600;
    host.commands.insert(USER.to_owned(), user);
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR)), Some("B"));
}

#[test]
fn a_read_on_its_own_thread_is_waited_for_when_it_ends_within_a_second() {
    let mut host = host();
    let state = ResolverState::new();
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &state, 0)), Some("A"));
    answers_after(&mut host, "B", 999);
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR)), Some("B"));
    answers_after(&mut host, "C", 1_000);
    let next = 2 * HALF_HOUR + 999;
    assert_eq!(model(&resolve(&host, &state, next)), Some("B"));
    assert_eq!(model(&resolve(&host, &state, next + 1_000)), Some("C"));
}

#[test]
fn a_fresh_state_reads_the_host_on_every_call() {
    let mut host = host();
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &ResolverState::new(), 0)), Some("A"));
    answers(&mut host, "B");
    assert_eq!(model(&resolve(&host, &ResolverState::new(), 1)), Some("B"));
}

#[test]
fn a_read_still_running_on_its_own_thread_leaves_the_reading_in_force_until_it_ends() {
    let mut host = host();
    let state = ResolverState::new();
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &state, 0)), Some("A"));
    answers(&mut host, "B");
    host.hold_commands();
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR)), Some("A"));
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 2_000)), Some("A"));
    host.release_commands();
    let mut seen = None;
    for _ in 0..500 {
        seen = model(&resolve(&host, &state, HALF_HOUR + 3_000)).map(str::to_owned);
        if seen.as_deref() == Some("B") {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(seen.as_deref(), Some("B"));
    assert_eq!(machine_reads(&host), 2);
}

#[test]
fn a_read_whose_thread_goes_away_counts_as_unanswered_and_is_retried_at_once() {
    let mut host = host();
    let state = ResolverState::new();
    answers(&mut host, "A");
    assert_eq!(model(&resolve(&host, &state, 0)), Some("A"));
    host.throws = true;
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR)), Some("A"));
    host.throws = false;
    answers(&mut host, "B");
    assert_eq!(model(&resolve(&host, &state, HALF_HOUR + 1)), Some("B"));
    assert_eq!(machine_reads(&host), 3);
}
