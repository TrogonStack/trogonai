use std::collections::BTreeSet;

use serde_json::Value;
use trogon_claude_agent_sdk::upstream::messages::{SDKActiveGoalMessage, SDKActiveGoalMessageType, SDKMessage};
use trogon_claude_agent_sdk::{JsonObject, Message};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const FIXTURES: &str = include_str!("fixtures/upstream_messages.json");

/// The members of the pinned `SDKMessage` union, in declaration order: sample `k` sets member `k`.
const MEMBERS: [&str; 40] = [
    "SDKAssistantMessage",
    "SDKUserMessage",
    "SDKUserMessageReplay",
    "SDKResultMessage",
    "SDKSystemMessage",
    "SDKPartialAssistantMessage",
    "SDKCompactBoundaryMessage",
    "SDKStatusMessage",
    "SDKAPIRetryMessage",
    "SDKControlRequestProgressMessage",
    "SDKModelRefusalFallbackMessage",
    "SDKModelRefusalNoFallbackMessage",
    "SDKLocalCommandOutputMessage",
    "SDKHookStartedMessage",
    "SDKHookProgressMessage",
    "SDKHookResponseMessage",
    "SDKPluginInstallMessage",
    "SDKToolProgressMessage",
    "SDKAuthStatusMessage",
    "SDKTaskNotificationMessage",
    "SDKTaskStartedMessage",
    "SDKTaskUpdatedMessage",
    "SDKTaskProgressMessage",
    "SDKBackgroundTasksChangedMessage",
    "SDKThinkingTokensMessage",
    "SDKSessionStateChangedMessage",
    "SDKWorkerShuttingDownMessage",
    "SDKCommandsChangedMessage",
    "SDKNotificationMessage",
    "SDKFilesPersistedEvent",
    "SDKToolUseSummaryMessage",
    "SDKMemoryRecallMessage",
    "SDKRateLimitEvent",
    "SDKElicitationCompleteMessage",
    "SDKPermissionDeniedMessage",
    "SDKPromptSuggestionMessage",
    "SDKMirrorErrorMessage",
    "SDKInformationalMessage",
    "SDKConversationResetMessage",
    "SDKSessionWritesLostMessage",
];

fn samples(root: &str) -> Result<Vec<Value>, Box<dyn std::error::Error>> {
    let mut fixtures: serde_json::Map<String, Value> = serde_json::from_str(FIXTURES)?;
    match fixtures.remove(root) {
        Some(Value::Array(samples)) => Ok(samples),
        _ => Err(format!("fixture root {root} is missing").into()),
    }
}

fn object(sample: &Value) -> Result<JsonObject, &'static str> {
    sample.as_object().cloned().ok_or("sample is not an object")
}

/// A message the client could not give a Rust struct, which is how `active_goal` and most `SDKMessage` members
/// reach the caller.
fn unknown(sample: &Value) -> Result<Message, &'static str> {
    let raw = object(sample)?;
    let mut data = raw.clone();
    let kind = match data.shift_remove("type") {
        Some(Value::String(kind)) => kind,
        _ => return Err("sample has no string type"),
    };
    Ok(Message::Unknown { kind, data, raw })
}

/// The member a decoded union holds, read from its `Debug` form.
fn member(typed: &SDKMessage) -> String {
    let debug = format!("{typed:?}");
    debug.split('(').next().unwrap_or_default().to_owned()
}

/// Every declared key decoded into a field of its own: an `extra` map holds only the one undeclared key the
/// fixtures put in objects whose declarations carry an index signature.
fn declared_keys_are_typed(debug: &str) -> bool {
    debug.match_indices("extra: {\"").all(|(index, _)| {
        debug
            .get(index + "extra: {".len()..)
            .is_some_and(|rest| rest.starts_with("\"extra_key\""))
    })
}

#[test]
fn fixtures_carry_one_sample_for_every_sdk_message_member() -> TestResult {
    let samples = samples("SDKMessage")?;
    assert_eq!(samples.len(), MEMBERS.len());
    let decoded = samples
        .iter()
        .map(|sample| serde_json::from_value::<SDKMessage>(sample.clone()).map(|typed| member(&typed)))
        .collect::<Result<BTreeSet<_>, _>>()?;
    let declared: BTreeSet<String> = MEMBERS.iter().map(|name| (*name).to_owned()).collect();
    assert_eq!(decoded, declared);
    Ok(())
}

#[test]
fn every_sdk_message_member_decodes_into_itself_and_round_trips() -> TestResult {
    for (sample, expected) in samples("SDKMessage")?.into_iter().zip(MEMBERS) {
        let typed: SDKMessage = serde_json::from_value(sample.clone())?;
        let debug = format!("{typed:?}");
        assert_eq!(member(&typed), expected, "{sample} decoded as {debug}");
        assert!(
            declared_keys_are_typed(&debug),
            "{expected} left declared keys in extra"
        );
        assert_eq!(serde_json::to_value(&typed)?, sample, "{expected} did not round-trip");
        let text = serde_json::to_string(&sample)?;
        let reparsed: SDKMessage = serde_json::from_str(&text)?;
        assert_eq!(
            serde_json::to_string(&reparsed)?,
            text,
            "{expected} did not re-encode byte-identically"
        );
    }
    Ok(())
}

#[test]
fn a_yielded_message_types_as_its_sdk_message_member() -> TestResult {
    for (sample, expected) in samples("SDKMessage")?.into_iter().zip(MEMBERS) {
        let typed = unknown(&sample)?.to_sdk_message();
        assert_eq!(member(&typed), expected, "{sample}");
        assert_eq!(serde_json::to_value(&typed)?, sample, "{expected}");
    }
    Ok(())
}

#[test]
fn hook_lifecycle_messages_are_typed() -> TestResult {
    let samples = samples("SDKMessage")?;
    for (index, expected) in [
        (13, "SDKHookStartedMessage"),
        (14, "SDKHookProgressMessage"),
        (15, "SDKHookResponseMessage"),
    ] {
        let sample = samples.get(index).ok_or("sample is missing")?;
        let typed = unknown(sample)?.to_sdk_message();
        match (&typed, expected) {
            (SDKMessage::SDKHookStartedMessage(message), "SDKHookStartedMessage") => {
                assert_eq!(message.hook_event, sample["hook_event"]);
            }
            (SDKMessage::SDKHookProgressMessage(message), "SDKHookProgressMessage") => {
                assert_eq!(message.stdout, sample["stdout"]);
            }
            (SDKMessage::SDKHookResponseMessage(message), "SDKHookResponseMessage") => {
                assert_eq!(serde_json::to_value(&message.outcome)?, sample["outcome"]);
            }
            _ => return Err(format!("{expected} decoded as {typed:?}").into()),
        }
    }
    Ok(())
}

#[test]
fn a_message_no_member_declares_stays_whole_as_unknown() -> TestResult {
    let sample: Value = serde_json::from_str(r#"{"type":"system","subtype":"future","novel":[1,null]}"#)?;
    let typed = unknown(&sample)?.to_sdk_message();
    assert_eq!(typed, SDKMessage::Unknown(sample.clone()));
    assert_eq!(serde_json::to_value(&typed)?, sample);
    Ok(())
}

#[test]
fn active_goal_messages_are_typed_and_round_trip() -> TestResult {
    let mut samples = samples("SDKActiveGoalMessage")?;
    assert!(!samples.is_empty());
    samples.push(serde_json::from_str(
        r#"{"type":"active_goal","value":null,"uuid":"u","session_id":"s"}"#,
    )?);
    for sample in samples {
        let typed = unknown(&sample)?
            .to_active_goal_message()
            .ok_or_else(|| format!("{sample} was not typed"))?;
        assert_eq!(typed.r#type, SDKActiveGoalMessageType::ActiveGoal);
        assert_eq!(typed.value.is_none(), sample["value"].is_null());
        assert!(declared_keys_are_typed(&format!("{typed:?}")), "{sample}");
        assert_eq!(serde_json::to_value(&typed)?, sample);
        let text = serde_json::to_string(&sample)?;
        let reparsed: SDKActiveGoalMessage = serde_json::from_str(&text)?;
        assert_eq!(serde_json::to_string(&reparsed)?, text);
    }
    Ok(())
}

#[test]
fn only_an_active_goal_message_types_as_one() -> TestResult {
    let samples = samples("SDKMessage")?;
    for sample in &samples {
        assert!(unknown(sample)?.to_active_goal_message().is_none(), "{sample}");
    }
    let future: Value = serde_json::from_str(r#"{"type":"future_goal","value":null,"uuid":"u","session_id":"s"}"#)?;
    assert!(unknown(&future)?.to_active_goal_message().is_none());
    Ok(())
}
