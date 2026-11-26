//! Replays the control requests of the goldens that wait for a host promise (`stop_task`, `background_tasks`,
//! `side_question`, and the promise verdicts of `set_model` and `set_max_thinking_tokens`) through [`AsyncControl`],
//! with the recorded host behaviour, and compares every frame the session wrote, the host calls, and the deadline.
//! Time is paused, so the 660 second side question deadline costs nothing.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::task::JoinHandle;
use trogon_claude_agent_sdk::{
    AsyncControl, AsyncControlCallbacks, BridgeControlReply, CallbackResult, CancellationToken, ControlCallbacks,
    ControlSession, HostError, HostFuture, ModelVerdict, PendingControl, SIDE_QUESTION_DEADLINE, SideQuestionContext,
    answer_control_request, background_tasks_disabled,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;
type Log = Arc<Mutex<Vec<Value>>>;

const GOLDENS: &str = include_str!("fixtures/bridge_worker/goldens.json");
const ASYNC_KEYS: [&str; 7] = [
    "resolves",
    "resolvesNothing",
    "rejects",
    "rejectsAbort",
    "never",
    "rejectsOnAbort",
    "progress",
];

fn push(log: &Log, value: Value) {
    log.lock().unwrap_or_else(PoisonError::into_inner).push(value);
}

fn snapshot(log: &Log) -> Vec<Value> {
    log.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

fn is_async(case: &Value) -> bool {
    case["callbacks"].as_object().is_some_and(|callbacks| {
        callbacks
            .values()
            .any(|behaviour| ASYNC_KEYS.iter().any(|key| behaviour.get(key).is_some()))
    })
}

/// What the recorded host does, replayed: settle at once, after a delay, never, or when told to stop.
fn settle<T: Send + 'static>(
    behaviour: &Value,
    signal: Option<CancellationToken>,
    answer: impl Fn(Option<&Value>) -> T + Send + 'static,
) -> HostFuture<T> {
    let behaviour = behaviour.clone();
    Box::pin(async move {
        if let Some(delay) = behaviour["resolvesAfterMs"].as_u64() {
            tokio::time::sleep(Duration::from_millis(delay)).await;
            return Ok(answer(behaviour.get("resolves")));
        }
        if let Some(message) = behaviour["rejects"].as_str() {
            return Err(HostError::Failed(message.to_owned()));
        }
        if let Some(message) = behaviour["rejectsAbort"].as_str() {
            return Err(HostError::Aborted(message.to_owned()));
        }
        if behaviour["never"] == true {
            return std::future::pending().await;
        }
        if let (true, Some(signal)) = (behaviour["rejectsOnAbort"] == true, signal) {
            signal.cancelled().await;
            return Err(HostError::Aborted("the host stopped".to_owned()));
        }
        Ok(answer(behaviour.get("resolves")))
    })
}

fn verdict(value: &Value) -> ModelVerdict {
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    ModelVerdict {
        ok: value["ok"] == true,
        model: text("model"),
        notices: value
            .get("notices")
            .and_then(Value::as_array)
            .map(|notices| notices.iter().filter_map(Value::as_str).map(str::to_owned).collect()),
        error: text("error"),
        code: text("code"),
    }
}

fn or_undefined(value: Option<&Value>) -> Value {
    value.cloned().unwrap_or_else(|| json!("<undefined>"))
}

struct Host {
    control: ControlCallbacks,
    asynchronous: AsyncControlCallbacks,
    calls: Log,
    signals: Arc<Mutex<Vec<CancellationToken>>>,
}

fn host(registered: &Value) -> Host {
    let calls = Log::default();
    let signals = Arc::new(Mutex::new(Vec::new()));
    let mut control = ControlCallbacks::default();
    let mut asynchronous = AsyncControlCallbacks::default();
    for (key, behaviour) in registered.as_object().into_iter().flatten() {
        let (log, behaviour) = (Arc::clone(&calls), behaviour.clone());
        match key.as_str() {
            "onSetModel" => {
                control.on_set_model_pending = Some(Box::new(move |model: Option<&str>| {
                    push(
                        &log,
                        json!(["onSetModel", model.map_or_else(|| json!("<undefined>"), |m| json!(m))]),
                    );
                    settle(&behaviour, None, |answer| answer.map(verdict))
                }));
            }
            "onSetMaxThinkingTokens" => {
                control.on_set_max_thinking_tokens_pending =
                    Some(Box::new(move |tokens: Option<&Value>, display: Option<&Value>| {
                        push(
                            &log,
                            json!(["onSetMaxThinkingTokens", or_undefined(tokens), or_undefined(display)]),
                        );
                        settle(&behaviour, None, |answer| {
                            answer.map(|answer| {
                                if answer["ok"] == false {
                                    CallbackResult::Failed {
                                        error: answer["error"].as_str().map(str::to_owned),
                                        code: None,
                                    }
                                } else {
                                    CallbackResult::Ok
                                }
                            })
                        })
                    }));
            }
            "onStopTask" => {
                control.has_stop_task = true;
                asynchronous.on_stop_task = Some(Box::new(move |task: &str| {
                    push(&log, json!(["onStopTask", task]));
                    settle(&behaviour, None, |answer| answer.cloned())
                }));
            }
            "onBackgroundTasks" => {
                control.has_background_tasks = true;
                asynchronous.on_background_tasks = Some(Box::new(move |tool_use_id: Option<&str>| {
                    push(
                        &log,
                        json!([
                            "onBackgroundTasks",
                            tool_use_id.map_or_else(|| json!("<undefined>"), |id| json!(id))
                        ]),
                    );
                    settle(&behaviour, None, |answer| answer.cloned())
                }));
            }
            "onSideQuestion" => {
                control.has_side_question = true;
                let signals = Arc::clone(&signals);
                asynchronous.on_side_question = Some(Box::new(move |data: &Value, context: SideQuestionContext| {
                    push(&log, json!(["onSideQuestion", data, "<options>"]));
                    signals
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(context.signal.clone());
                    for update in behaviour["progress"].as_array().into_iter().flatten() {
                        context.report_progress(update.as_object().cloned().unwrap_or_default());
                    }
                    settle(&behaviour, Some(context.signal.clone()), |answer| {
                        answer.and_then(Value::as_object).cloned()
                    })
                }));
            }
            _ => {}
        }
    }
    Host {
        control,
        asynchronous,
        calls,
        signals,
    }
}

async fn let_tasks_run() {
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
}

struct Session {
    control: AsyncControl,
    callbacks: ControlCallbacks,
    version: Option<String>,
    tasks: Vec<JoinHandle<()>>,
    /// Where the attach path queues the replies the dispatcher builds on the spot.
    written: Log,
}

impl Session {
    /// Dispatches one client frame the way the attach path does.
    fn deliver(&mut self, frame: &Value) {
        if frame["type"] == "control_cancel_request" {
            self.control.cancel(&frame["request_id"]);
            return;
        }
        let session = ControlSession {
            session_id: "cse_abc",
            claude_code_version: self.version.as_deref(),
            pid: 0,
            pending_prompts: &[],
        };
        let task = match answer_control_request(frame, &session, &mut self.callbacks) {
            BridgeControlReply::Frame(reply) => {
                push(&self.written, reply);
                None
            }
            BridgeControlReply::Deferred(work) => self.control.start(frame, work),
            BridgeControlReply::Pending(pending) => Some(self.control.finish_pending(frame, pending)),
        };
        self.tasks.extend(task.map(tokio::spawn));
    }
}

#[tokio::test(start_paused = true)]
async fn host_promises_are_answered_as_the_recorded_session_answered() -> TestResult {
    let goldens: Value = serde_json::from_str(GOLDENS)?;
    let cases: Vec<&Value> = goldens["control"]
        .as_array()
        .ok_or("no control cases")?
        .iter()
        .filter(|case| is_async(case))
        .collect();
    assert!(cases.len() > 50, "{}", cases.len());
    for case in cases {
        let name = case["name"].as_str().ok_or("unnamed case")?;
        let written = Log::default();
        let host = host(&case["callbacks"]);
        let env: Map<String, Value> = case["env"].as_object().cloned().unwrap_or_default();
        let sink = Arc::clone(&written);
        let control = AsyncControl::new(
            "cse_abc",
            Box::new(move |frame| push(&sink, frame)),
            Box::new(|| "<uuid>".to_owned()),
            host.asynchronous,
            Box::new(move || {
                background_tasks_disabled(|name| env.get(name).and_then(Value::as_str).map(str::to_owned), &[])
            }),
        );
        let mut session = Session {
            control,
            callbacks: host.control,
            version: case["version"].as_str().map(str::to_owned),
            tasks: Vec::new(),
            written: Arc::clone(&written),
        };
        session.deliver(&case["request"]);
        let_tasks_run().await;
        for later in case["later"].as_array().into_iter().flatten() {
            tokio::time::advance(Duration::from_millis(200)).await;
            let_tasks_run().await;
            session.deliver(later);
            let_tasks_run().await;
        }
        tokio::time::advance(SIDE_QUESTION_DEADLINE + Duration::from_millis(1)).await;
        for task in session.tasks {
            task.await?;
        }
        let mut expected = case["frames"].as_array().ok_or("no frames")?.clone();
        for frame in &mut expected {
            if frame["type"] == "control_response"
                && let Some(frame) = frame.as_object_mut()
            {
                frame.shift_remove("uuid");
            }
        }
        // The recorder keeps only the `control_response` and `system` payloads the session posted.
        let posted: Vec<Value> = snapshot(&written)
            .into_iter()
            .filter(|frame| frame["type"] == "control_response" || frame["type"] == "system")
            .collect();
        assert_eq!(
            serde_json::to_string(&expected)?,
            serde_json::to_string(&posted)?,
            "{name}"
        );
        let mut calls = snapshot(&host.calls);
        let signals = host.signals.lock().unwrap_or_else(PoisonError::into_inner);
        if !signals.is_empty() {
            let aborted: Vec<Value> = signals.iter().map(|signal| json!(signal.is_cancelled())).collect();
            calls.push(json!(["<signals aborted>", aborted]));
        }
        assert_eq!(Value::Array(calls), case["calls"], "{name}");
        let deadlines: Vec<Value> = if signals.is_empty() {
            Vec::new()
        } else {
            vec![json!(SIDE_QUESTION_DEADLINE.as_millis())]
        };
        assert_eq!(Value::Array(deadlines), case["deadlines"], "{name}");
    }
    Ok(())
}

fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let pairs: Vec<(String, String)> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    move |name| {
        pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    }
}

#[test]
fn background_tasks_are_off_for_a_switched_on_flag_or_a_bare_run() {
    let none: [String; 0] = [];
    assert!(!background_tasks_disabled(environment(&[]), &none));
    assert!(!background_tasks_disabled(
        environment(&[("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "0")]),
        &none
    ));
    for word in ["1", "true", " Yes ", "ON"] {
        assert!(background_tasks_disabled(
            environment(&[("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", word)]),
            &none
        ));
        assert!(background_tasks_disabled(
            environment(&[("CLAUDE_CODE_SIMPLE", word)]),
            &none
        ));
    }
    let bare = ["run".to_owned(), "--bare".to_owned()];
    assert!(background_tasks_disabled(environment(&[]), &bare));
    let after_separator = ["run".to_owned(), "--".to_owned(), "--bare".to_owned()];
    assert!(!background_tasks_disabled(environment(&[]), &after_separator));
}

fn quiet_control() -> AsyncControl {
    AsyncControl::new(
        "cse_abc",
        Box::new(|_| {}),
        Box::new(|| "<uuid>".to_owned()),
        AsyncControlCallbacks::default(),
        Box::new(|| false),
    )
}

#[test]
fn a_cancel_for_a_request_that_is_not_open_is_ignored() {
    quiet_control().cancel(&json!("never-started"));
}

#[tokio::test]
async fn a_pending_verdict_is_answered_when_the_host_settles() {
    let written = Log::default();
    let sink = Arc::clone(&written);
    let control = AsyncControl::new(
        "cse_abc",
        Box::new(move |frame| push(&sink, frame)),
        Box::new(|| "<uuid>".to_owned()),
        AsyncControlCallbacks::default(),
        Box::new(|| false),
    );
    let request = json!({ "type": "control_request", "request_id": "r9" });
    control
        .finish_pending(
            &request,
            PendingControl::SetModel(Box::pin(async { Ok::<_, HostError>(None) })),
        )
        .await;
    assert_eq!(
        snapshot(&written),
        vec![json!({
            "type": "control_response",
            "response": { "subtype": "success", "request_id": "r9" },
            "session_id": "cse_abc",
        })]
    );
}

fn simple_switch(value: &str) -> impl Fn(&str) -> Option<String> + '_ {
    move |name| (name == "CLAUDE_CODE_SIMPLE").then(|| value.to_owned())
}

#[test]
fn background_task_switches_trim_like_javascript() {
    for value in ["\u{feff}1\u{feff}", "\u{feff}true"] {
        assert!(background_tasks_disabled(simple_switch(value), &[]), "{value:?}");
    }
    for value in ["\u{85}1\u{85}", "\u{85}true"] {
        assert!(!background_tasks_disabled(simple_switch(value), &[]), "{value:?}");
    }
}
