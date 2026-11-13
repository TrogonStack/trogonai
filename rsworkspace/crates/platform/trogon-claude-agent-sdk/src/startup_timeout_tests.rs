use std::sync::{Arc, Mutex};

use super::*;
use crate::upstream::messages::SDKStartupFailureReason;

fn at(base: Instant, millis: u64) -> Instant {
    base + Duration::from_millis(millis)
}

fn started(event: &str, id: &str) -> StartupEvent {
    StartupEvent::HookStarted {
        event: event.to_owned(),
        id: id.to_owned(),
    }
}

fn responded(event: &str, id: &str) -> StartupEvent {
    StartupEvent::HookResponse {
        event: event.to_owned(),
        id: id.to_owned(),
    }
}

fn expired(timer: &mut StartupTimer, now: Instant) -> Result<InitializeTimeoutError, &'static str> {
    timer.fire(now).ok_or("the timer did not expire")
}

const SILENT_TAIL: &str =
    "Slow plugins, MCP servers, hooks that Claude Code does not report, disks or network can each cause this.";

#[test]
fn a_passed_limit_counts_hook_time() -> Result<(), &'static str> {
    let base = Instant::now();
    let mut timer = StartupTimer::new(Some(Duration::from_millis(400)), base, None);
    timer.note(&started("SessionStart", "h1"), at(base, 100));
    assert_eq!(timer.deadline(), at(base, 400));
    let error = expired(&mut timer, at(base, 400))?;
    assert_eq!(
        error.message(),
        "Subprocess initialization did not complete within 400ms. Claude Code reported 1 SessionStart hook, which had not finished, and did not answer the initialize request. A hook that runs in the background counts as not finished until it ends. Check this hook first, then authentication and network connectivity."
    );
    assert_eq!(error.startup_hooks_open(), 1);
    assert_eq!(
        error.telemetry_message(),
        "Subprocess initialization did not complete within 400ms"
    );
    Ok(())
}

#[test]
fn the_default_limit_does_not_count_hook_time() -> Result<(), &'static str> {
    let base = Instant::now();
    let mut timer = StartupTimer::new(None, base, None);
    timer.note(&started("SessionStart", "h1"), at(base, 10_000));
    assert_eq!(timer.deadline(), at(base, 610_000));
    timer.note(&responded("SessionStart", "h1"), at(base, 70_000));
    assert_eq!(timer.deadline(), at(base, 120_000));
    let error = expired(&mut timer, at(base, 120_000))?;
    assert_eq!(
        error.message(),
        format!(
            "Subprocess initialization did not complete within 60000ms. Not counted: 60 seconds in which Claude Code reported a SessionStart or Setup hook running. Claude Code reported 1 SessionStart hook, which finished, and then did not answer the initialize request. {SILENT_TAIL}"
        )
    );
    assert_eq!(error.startup_hooks_open(), 0);
    Ok(())
}

#[test]
fn uncounted_time_stops_at_ten_minutes() -> Result<(), &'static str> {
    let base = Instant::now();
    let mut timer = StartupTimer::new(None, base, None);
    timer.note(&started("Setup", "h1"), base);
    assert_eq!(timer.deadline(), at(base, 600_000));
    assert!(timer.fire(at(base, 600_000)).is_none());
    assert_eq!(timer.deadline(), at(base, 660_000));
    let error = expired(&mut timer, at(base, 660_000))?;
    assert!(
        error.message().contains("Not counted: 600 seconds in which"),
        "{}",
        error.message()
    );
    assert!(error.message().contains("1 Setup hook, which had not finished"));
    Ok(())
}

#[test]
fn uncounted_seconds_round_half_up() -> Result<(), &'static str> {
    for (millis, expected) in [(499_u64, None), (500, Some("1 second")), (1_500, Some("2 seconds"))] {
        let base = Instant::now();
        let mut timer = StartupTimer::new(None, base, None);
        timer.note(&started("Setup", "h1"), base);
        timer.note(&responded("Setup", "h1"), at(base, millis));
        let deadline = timer.deadline();
        let error = expired(&mut timer, deadline)?;
        match expected {
            Some(text) => assert!(
                error.message().contains(&format!("Not counted: {text} in which")),
                "{}",
                error.message()
            ),
            None => assert!(!error.message().contains("Not counted")),
        }
    }
    Ok(())
}

#[test]
fn hook_summaries_follow_the_upstream_wording() -> Result<(), &'static str> {
    let limit = Some(Duration::from_millis(400));
    let base = Instant::now();
    let message = |events: &[StartupEvent]| -> Result<String, &'static str> {
        let mut timer = StartupTimer::new(limit, base, None);
        for event in events {
            timer.note(event, base);
        }
        Ok(expired(&mut timer, at(base, 400))?.message().to_owned())
    };
    assert_eq!(
        message(&[started("Setup", "h1"), responded("Setup", "h1")])?,
        format!(
            "Subprocess initialization did not complete within 400ms. Claude Code reported 1 Setup hook, which finished, and then did not answer the initialize request. {SILENT_TAIL}"
        )
    );
    assert_eq!(
        message(&[
            started("Setup", "h1"),
            started("Setup", "h2"),
            responded("Setup", "h1"),
            responded("Setup", "h2")
        ])?,
        format!(
            "Subprocess initialization did not complete within 400ms. Claude Code reported 2 Setup hooks, which all finished, and then did not answer the initialize request. {SILENT_TAIL}"
        )
    );
    assert_eq!(
        message(&[started("SessionStart", "h1"), started("Setup", "h2")])?,
        "Subprocess initialization did not complete within 400ms. Claude Code reported 2 SessionStart and Setup hooks, none of which had finished, and did not answer the initialize request. A hook that runs in the background counts as not finished until it ends. Check these hooks first, then authentication and network connectivity."
    );
    assert_eq!(
        message(&[
            started("Setup", "h1"),
            started("SessionStart", "h2"),
            started("Setup", "h3"),
            responded("SessionStart", "h2")
        ])?,
        "Subprocess initialization did not complete within 400ms. Claude Code reported 3 Setup and SessionStart hooks, 2 of which had not finished, and did not answer the initialize request. A hook that runs in the background counts as not finished until it ends. Check these hooks first, then authentication and network connectivity."
    );
    assert_eq!(
        message(&[started("PreToolUse", "h1")])?,
        format!(
            "Subprocess initialization did not complete within 400ms. Claude Code did not answer the initialize request and reported no SessionStart or Setup hook running. {SILENT_TAIL}"
        )
    );
    Ok(())
}

#[test]
fn hooks_past_the_followed_limit_are_only_counted() {
    let base = Instant::now();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    let callback: StartupHooksRunning = Arc::new(move |open| {
        sink.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(open);
    });
    let mut timer = StartupTimer::new(None, base, Some(callback));
    for index in 0..66 {
        timer.note(&started("SessionStart", &format!("h{index}")), base);
    }
    // An unknown id of a startup event closes one of the unfollowed hooks, an unknown event closes nothing.
    timer.note(&responded("PreToolUse", "stray"), base);
    timer.note(&responded("SessionStart", "stray"), base);
    timer.note(&responded("SessionStart", "h0"), base);
    // A hook that is already followed is not counted twice.
    timer.note(&started("SessionStart", "h1"), base);
    let seen = seen.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    assert_eq!(seen.first(), Some(&1));
    assert_eq!(seen.get(65), Some(&66));
    assert_eq!(seen.get(66), Some(&65));
    assert_eq!(seen.get(67), Some(&64));
    assert_eq!(seen.len(), 68);
}

fn frame(value: &str) -> crate::JsonObject {
    serde_json::from_str(value).unwrap_or_default()
}

#[test]
fn frames_map_to_startup_events() {
    let now = Instant::now();
    let mark = |name: &str| {
        StartupEvent::from_frame(
            &frame(&format!(r#"{{"type":"keep_alive","startup_mark":"{name}"}}"#)),
            now,
        )
    };
    assert!(matches!(mark("loaded"), Some(StartupEvent::Mark { mark, .. }) if mark == "loaded"));
    assert!(mark(&"a".repeat(64)).is_some());
    assert!(mark(&"a".repeat(65)).is_none());
    // JavaScript counts UTF-16 code units: 32 emoji are 64 units, 33 are 66.
    assert!(mark(&"\u{1F600}".repeat(32)).is_some());
    assert!(mark(&"\u{1F600}".repeat(33)).is_none());
    assert!(StartupEvent::from_frame(&frame(r#"{"type":"keep_alive"}"#), now).is_none());
    assert!(StartupEvent::from_frame(&frame(r#"{"type":"keep_alive","startup_mark":7}"#), now).is_none());
    assert!(StartupEvent::from_frame(&frame(r#"{"type":"assistant"}"#), now).is_none());
    assert!(StartupEvent::from_frame(&frame(r#"{"type":"system","subtype":"init"}"#), now).is_none());
    let hook = StartupEvent::from_frame(
        &frame(r#"{"type":"system","subtype":"hook_started","hook_id":"x","hook_event":"Setup"}"#),
        now,
    );
    assert!(matches!(hook, Some(StartupEvent::HookStarted { event, id }) if event == "Setup" && id == "\"x\""));
    let done = StartupEvent::from_frame(
        &frame(r#"{"type":"system","subtype":"hook_response","hook_event":"Setup"}"#),
        now,
    );
    assert!(matches!(done, Some(StartupEvent::HookResponse { id, .. }) if id == "undefined"));
}

#[test]
fn an_error_result_exposes_the_startup_failure_reason() {
    let error = |raw: &str| crate::ResultError::new("boom".to_owned(), frame(raw), Some(1));
    assert_eq!(
        error(r#"{"type":"result","startup_failure_reason":"cwd_unavailable"}"#).startup_failure_reason(),
        Some(SDKStartupFailureReason::CwdUnavailable)
    );
    assert_eq!(
        error(r#"{"type":"result","startup_failure_reason":7}"#).startup_failure_reason(),
        None
    );
    assert_eq!(error(r#"{"type":"result"}"#).startup_failure_reason(), None);
}
