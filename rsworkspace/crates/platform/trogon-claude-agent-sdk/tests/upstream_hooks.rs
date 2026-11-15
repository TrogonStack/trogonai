use std::collections::BTreeSet;

use serde_json::Value;
use trogon_claude_agent_sdk::upstream::hooks::{
    AsyncHookJSONOutput, SyncHookJSONOutput, SyncHookJSONOutputDecision, SyncHookJSONOutputHookSpecificOutput,
};
use trogon_claude_agent_sdk::{
    CancellationToken, EXIT_REASONS, ExitReason, HOOK_EVENTS, HookEvent, HookInput, HookJSONOutput, HookOutput,
    HookRequest,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FIXTURES: &str = include_str!("fixtures/upstream_hooks.json");

fn samples(root: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut fixtures: serde_json::Map<String, Value> = serde_json::from_str(FIXTURES)?;
    match fixtures.remove(root) {
        Some(Value::Array(samples)) => Ok(samples),
        _ => Err(format!("fixture root {root} is missing").into()),
    }
}

fn event_name(sample: &Value) -> Result<&str, &'static str> {
    sample
        .get("hook_event_name")
        .and_then(Value::as_str)
        .ok_or("sample has no hook_event_name")
}

fn request(sample: &Value) -> Result<HookRequest, &'static str> {
    Ok(HookRequest {
        input: sample.as_object().ok_or("sample is not an object")?.clone(),
        tool_use_id: None,
        signal: CancellationToken::new(),
    })
}

/// Every declared key decoded into a field of its own when no `extra` map holds anything.
fn no_key_left_in_extra(debug: &str) -> bool {
    !debug.contains("extra: {\"")
}

#[test]
fn the_event_list_matches_the_upstream_hook_events_constant() {
    assert_eq!(HOOK_EVENTS.len(), 33);
    assert_eq!(HookEvent::ALL.map(HookEvent::as_str), HOOK_EVENTS);
}

#[test]
fn exit_reasons_match_the_upstream_constant_and_round_trip() -> TestResult {
    assert_eq!(
        EXIT_REASONS,
        ["clear", "resume", "logout", "prompt_input_exit", "other"]
    );
    for reason in EXIT_REASONS {
        let typed: ExitReason = serde_json::from_value(Value::String(reason.to_owned()))?;
        assert!(
            !matches!(typed, ExitReason::Unknown(_)),
            "{reason} is not a declared exit reason"
        );
        assert_eq!(typed.as_str(), reason);
    }
    let future: ExitReason = serde_json::from_str(r#""future_reason""#)?;
    assert_eq!(future, ExitReason::Unknown("future_reason".to_owned()));
    assert_eq!(serde_json::to_string(&future)?, r#""future_reason""#);
    Ok(())
}

#[test]
fn fixtures_carry_one_input_for_every_upstream_hook_event() -> TestResult {
    let covered = samples("HookInput")?
        .iter()
        .map(|sample| event_name(sample).map(str::to_owned))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let declared: BTreeSet<String> = HOOK_EVENTS.iter().map(|event| (*event).to_owned()).collect();
    assert_eq!(covered, declared);
    Ok(())
}

#[test]
fn every_hook_input_decodes_into_its_own_event_and_round_trips() -> TestResult {
    for sample in samples("HookInput")? {
        let event = event_name(&sample)?;
        let typed: HookInput = serde_json::from_value(sample.clone())?;
        let debug = format!("{typed:?}");
        assert!(
            debug.starts_with(&format!("{event}HookInput(")),
            "{event} decoded as {debug}"
        );
        assert!(no_key_left_in_extra(&debug), "{event} left declared keys in extra");
        assert_eq!(serde_json::to_value(&typed)?, sample, "{event} did not round-trip");
        let text = serde_json::to_string(&sample)?;
        let reparsed: HookInput = serde_json::from_str(&text)?;
        assert_eq!(
            serde_json::to_string(&reparsed)?,
            text,
            "{event} did not re-encode byte-identically"
        );
    }
    Ok(())
}

#[test]
fn a_hook_request_exposes_its_input_as_the_upstream_union() -> TestResult {
    for sample in samples("HookInput")? {
        let event = event_name(&sample)?;
        let typed = request(&sample)?.typed_input();
        assert!(
            format!("{typed:?}").starts_with(&format!("{event}HookInput(")),
            "{event} was not typed"
        );
        assert_eq!(serde_json::to_value(&typed)?, sample);
    }
    Ok(())
}

#[test]
fn an_input_no_event_declares_stays_whole_as_unknown() -> TestResult {
    let sample: Value = serde_json::from_str(r#"{"hook_event_name":"FutureEvent","session_id":"s","novel":[1]}"#)?;
    let typed = request(&sample)?.typed_input();
    assert_eq!(typed, HookInput::Unknown(sample.clone()));
    assert_eq!(serde_json::to_value(&typed)?, sample);
    Ok(())
}

#[test]
fn an_input_with_undeclared_keys_keeps_them_in_order() -> TestResult {
    let text = r#"{"session_id":"s","future":{"b":1,"a":2},"transcript_path":"t","cwd":"c","hook_event_name":"Stop","stop_hook_active":false}"#;
    let typed: HookInput = serde_json::from_str(text)?;
    match &typed {
        HookInput::StopHookInput(input) => {
            assert!(!input.stop_hook_active);
            assert_eq!(input.extra.keys().map(String::as_str).collect::<Vec<_>>(), ["future"]);
        }
        other => return Err(format!("decoded as {other:?}").into()),
    }
    assert_eq!(serde_json::to_string(&typed)?, text);
    Ok(())
}

#[test]
fn fixtures_carry_one_sync_output_for_every_hook_specific_output() -> TestResult {
    let samples = samples("SyncHookJSONOutput")?;
    assert_eq!(samples.len(), 22);
    for sample in samples {
        let typed: SyncHookJSONOutput = serde_json::from_value(sample.clone())?;
        let specific = typed
            .hook_specific_output
            .as_ref()
            .ok_or("sample has no hookSpecificOutput")?;
        assert!(
            !matches!(specific, SyncHookJSONOutputHookSpecificOutput::Unknown(_)),
            "{sample} hookSpecificOutput did not decode"
        );
        let event = sample
            .pointer("/hookSpecificOutput/hookEventName")
            .and_then(Value::as_str)
            .ok_or("hookSpecificOutput has no hookEventName")?;
        assert!(
            format!("{specific:?}").starts_with(&format!("{event}HookSpecificOutput(")),
            "{event} specific output decoded as {specific:?}"
        );
        assert!(
            no_key_left_in_extra(&format!("{typed:?}")),
            "{event} left declared keys in extra"
        );
    }
    Ok(())
}

#[test]
fn a_specific_output_no_event_declares_stays_whole_as_unknown() -> TestResult {
    let sample: Value =
        serde_json::from_str(r#"{"hookSpecificOutput":{"hookEventName":"FutureEvent","additionalContext":"c"}}"#)?;
    let typed: SyncHookJSONOutput = serde_json::from_value(sample.clone())?;
    let specific = sample
        .get("hookSpecificOutput")
        .ok_or("sample has no hookSpecificOutput")?;
    assert_eq!(
        typed.hook_specific_output,
        Some(SyncHookJSONOutputHookSpecificOutput::Unknown(specific.clone()))
    );
    assert_eq!(serde_json::to_value(&typed)?, sample);
    Ok(())
}

#[test]
fn every_sync_output_is_sent_byte_for_byte_as_given() -> TestResult {
    for sample in samples("SyncHookJSONOutput")? {
        let typed: HookJSONOutput = serde_json::from_value(sample.clone())?;
        assert!(
            matches!(typed, HookJSONOutput::SyncHookJSONOutput(_)),
            "{sample} decoded as {typed:?}"
        );
        assert_eq!(HookOutput::from(typed).to_wire_json()?, sample);
    }
    Ok(())
}

#[test]
fn both_upstream_decisions_are_typed() -> TestResult {
    let mut decisions = BTreeSet::new();
    for sample in samples("SyncHookJSONOutput")? {
        let typed: SyncHookJSONOutput = serde_json::from_value(sample)?;
        if let Some(decision) = typed.decision {
            assert!(!matches!(decision, SyncHookJSONOutputDecision::Unknown(_)));
            decisions.insert(decision.as_str().to_owned());
        }
    }
    assert_eq!(decisions, BTreeSet::from(["approve".to_owned(), "block".to_owned()]));
    Ok(())
}

#[test]
fn every_async_output_is_sent_byte_for_byte_as_given() -> TestResult {
    let samples = samples("AsyncHookJSONOutput")?;
    assert!(!samples.is_empty());
    for sample in samples {
        let typed: HookJSONOutput = serde_json::from_value(sample.clone())?;
        match &typed {
            HookJSONOutput::AsyncHookJSONOutput(AsyncHookJSONOutput { r#async, .. }) => assert!(*r#async),
            other => return Err(format!("{sample} decoded as {other:?}").into()),
        }
        assert_eq!(HookOutput::from(typed).to_wire_json()?, sample);
    }
    Ok(())
}

#[test]
fn an_output_no_member_declares_is_sent_unchanged() -> TestResult {
    let text = r#"{"continue":"maybe","future":[1]}"#;
    let typed: HookJSONOutput = serde_json::from_str(text)?;
    assert!(matches!(typed, HookJSONOutput::Unknown(_)), "{typed:?}");
    assert_eq!(serde_json::to_string(&HookOutput::from(typed).to_wire_json()?)?, text);
    Ok(())
}
