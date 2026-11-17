//! Replays the recorded control dispatch (`control` and `pure.sanitize` of the goldens, written by
//! `.config/mise/lib/claude-sdk-freshness/bridge_worker_goldens.mjs` through the bundle's real session and its own
//! `Kt`) through [`answer_control_request`] and [`clean_control_text`].

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use trogon_claude_agent_sdk::{
    BridgeControlReply, CallbackResult, ControlCallbacks, ControlSession, DeferredControl, HostError, PendingControl,
    answer_control_request, clean_control_text, is_pending_prompt,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Calls = Arc<Mutex<Vec<Value>>>;

/// The host behaviours that settle a promise; those cases are replayed by `bridge_worker_control_async`.
const PROMISE_KEYS: [&str; 7] = [
    "resolves",
    "resolvesNothing",
    "rejects",
    "rejectsAbort",
    "never",
    "rejectsOnAbort",
    "progress",
];

const GOLDENS: &str = include_str!("fixtures/bridge_worker/goldens.json");

fn goldens() -> Result<Value, serde_json::Error> {
    serde_json::from_str(GOLDENS)
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing {key} in {value}"))
}

fn list<'a>(value: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing {key} in {value}"))
}

/// What the recorded callback returned: nothing when the case names no `returns`.
fn returned(behaviour: &Value) -> Option<CallbackResult> {
    let result = behaviour.get("returns")?;
    Some(if result["ok"] == true {
        CallbackResult::Ok
    } else {
        CallbackResult::Failed {
            error: result.get("error").and_then(Value::as_str).map(str::to_owned),
            code: result.get("code").and_then(Value::as_str).map(str::to_owned),
        }
    })
}

fn record(calls: &Calls, call: Value) {
    if let Ok(mut calls) = calls.lock() {
        calls.push(call);
    }
}

fn or_undefined(value: Option<&Value>) -> Value {
    value.cloned().unwrap_or_else(|| json!("<undefined>"))
}

fn callbacks_from(spec: &Value, calls: &Calls) -> ControlCallbacks {
    let mut callbacks = ControlCallbacks::default();
    let Some(registered) = spec.as_object() else {
        return callbacks;
    };
    for (key, behaviour) in registered {
        let (calls, behaviour) = (Arc::clone(calls), behaviour.clone());
        match key.as_str() {
            "onInterrupt" => {
                callbacks.on_interrupt = Some(Box::new(move || record(&calls, json!(["onInterrupt"]))));
            }
            "onSetModel" => {
                callbacks.on_set_model = Some(Box::new(move |model: Option<&str>| {
                    record(
                        &calls,
                        json!(["onSetModel", model.map_or_else(|| json!("<undefined>"), |m| json!(m))]),
                    );
                    returned(&behaviour)
                }));
            }
            "onSetMaxThinkingTokens" => {
                callbacks.on_set_max_thinking_tokens =
                    Some(Box::new(move |tokens: Option<&Value>, display: Option<&Value>| {
                        record(
                            &calls,
                            json!(["onSetMaxThinkingTokens", or_undefined(tokens), or_undefined(display)]),
                        );
                        returned(&behaviour)
                    }));
            }
            "onSetPermissionMode" => {
                callbacks.on_set_permission_mode = Some(Box::new(move |mode: &str| {
                    record(&calls, json!(["onSetPermissionMode", mode]));
                    returned(&behaviour)
                }));
            }
            "onRenameSession" => {
                callbacks.on_rename_session = Some(Box::new(move |title: &str| {
                    record(&calls, json!(["onRenameSession", title]));
                    returned(&behaviour)
                }));
            }
            "onStopTask" => callbacks.has_stop_task = true,
            "onBackgroundTasks" => callbacks.has_background_tasks = true,
            "onSideQuestion" => callbacks.has_side_question = true,
            _ => {}
        }
    }
    callbacks
}

#[test]
fn the_error_text_cleaner_is_the_recorded_one() -> TestResult {
    let goldens = goldens()?;
    let table = list(&goldens["pure"], "sanitize")?;
    assert!(!table.is_empty());
    for row in table {
        assert_eq!(text(row, "output")?, clean_control_text(text(row, "input")?), "{row}");
    }
    Ok(())
}

#[test]
fn control_requests_are_answered_as_the_recorded_session_answered() -> TestResult {
    let goldens = goldens()?;
    let cases = list(&goldens, "control")?;
    let mut replayed = 0;
    for case in cases {
        let waits_for_a_promise = case["callbacks"].as_object().is_some_and(|callbacks| {
            callbacks
                .values()
                .any(|behaviour| PROMISE_KEYS.iter().any(|key| behaviour.get(key).is_some()))
        });
        if waits_for_a_promise {
            continue;
        }
        replayed += 1;
        let name = text(case, "name")?;
        let calls = Calls::default();
        let mut callbacks = callbacks_from(&case["callbacks"], &calls);
        let prompts: Vec<Value> = list(case, "prompts")?
            .iter()
            .filter(|prompt| is_pending_prompt(prompt))
            .cloned()
            .collect();
        let session = ControlSession {
            session_id: "cse_abc",
            claude_code_version: case["version"].as_str(),
            pid: 0,
            pending_prompts: &prompts,
        };
        let reply = answer_control_request(&case["request"], &session, &mut callbacks);
        let mut expected = list(case, "frames")?
            .first()
            .cloned()
            .ok_or_else(|| format!("{name}: no recorded frame"))?;
        expected
            .as_object_mut()
            .ok_or_else(|| format!("{name}: frame is not an object"))?
            .shift_remove("uuid");
        let BridgeControlReply::Frame(frame) = reply else {
            return Err(format!("{name}: expected a frame").into());
        };
        assert_eq!(
            serde_json::to_string(&expected)?,
            serde_json::to_string(&frame)?,
            "{name}"
        );
        let seen = calls.lock().map_err(|error| error.to_string())?.clone();
        assert_eq!(Value::Array(seen), case["calls"], "{name}");
    }
    assert!(replayed > 90, "{replayed}");
    Ok(())
}

fn request(subtype: &str, extra: Value) -> Value {
    let mut body = json!({ "subtype": subtype });
    if let (Some(body), Some(extra)) = (body.as_object_mut(), extra.as_object()) {
        body.extend(extra.clone());
    }
    json!({ "type": "control_request", "request_id": "r1", "request": body })
}

fn deferred(request: &Value, callbacks: &mut ControlCallbacks) -> Option<DeferredControl> {
    let session = ControlSession {
        session_id: "cse_abc",
        claude_code_version: None,
        pid: 0,
        pending_prompts: &[],
    };
    match answer_control_request(request, &session, callbacks) {
        BridgeControlReply::Deferred(work) => Some(work),
        BridgeControlReply::Frame(_) | BridgeControlReply::Pending(_) => None,
    }
}

#[test]
fn requests_that_wait_for_a_host_call_are_handed_back_with_their_arguments() {
    let mut callbacks = ControlCallbacks {
        has_stop_task: true,
        has_background_tasks: true,
        has_side_question: true,
        ..ControlCallbacks::default()
    };
    assert_eq!(
        deferred(&request("stop_task", json!({ "task_id": "t1" })), &mut callbacks),
        Some(DeferredControl::StopTask {
            task_id: "t1".to_owned()
        })
    );
    assert_eq!(
        deferred(
            &request("background_tasks", json!({ "tool_use_id": "tu1" })),
            &mut callbacks
        ),
        Some(DeferredControl::BackgroundTasks {
            tool_use_id: Some("tu1".to_owned())
        })
    );
    for empty in [json!({}), json!({ "tool_use_id": null }), json!({ "tool_use_id": "" })] {
        assert_eq!(
            deferred(&request("background_tasks", empty), &mut callbacks),
            Some(DeferredControl::BackgroundTasks { tool_use_id: None })
        );
    }
    let question = request(
        "side_question",
        json!({ "question": "q", "history": [{ "question": "a", "response": "b", "fallback_notice": "n" }] }),
    );
    assert_eq!(
        deferred(&question, &mut callbacks),
        Some(DeferredControl::SideQuestion {
            request: question["request"].clone()
        })
    );
}

#[test]
fn only_permission_and_dialog_requests_stay_pending() {
    assert!(is_pending_prompt(&request("can_use_tool", json!({}))));
    assert!(is_pending_prompt(&request("request_user_dialog", json!({}))));
    assert!(!is_pending_prompt(&request("interrupt", json!({}))));
    assert!(!is_pending_prompt(&json!({ "type": "control_request" })));
}

#[test]
fn host_errors_and_pending_replies_describe_themselves() {
    assert_eq!(HostError::Aborted("stopped".to_owned()).message(), "stopped");
    assert_eq!(HostError::Failed("broke".to_owned()).message(), "broke");
    let model = PendingControl::SetModel(Box::pin(async { Ok::<_, HostError>(None) }));
    assert_eq!(format!("{model:?}"), "PendingControl::SetModel");
    let tokens = PendingControl::SetMaxThinkingTokens(Box::pin(async { Ok::<_, HostError>(None) }));
    assert_eq!(format!("{tokens:?}"), "PendingControl::SetMaxThinkingTokens");
}
