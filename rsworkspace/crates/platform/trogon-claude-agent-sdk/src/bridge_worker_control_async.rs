//! The replies of the bridge worker's control dispatch that wait for a host promise: `stop_task`, `background_tasks`,
//! `side_question` (with its abort, deadline and progress frames) and the promise verdicts of `set_model` and
//! `set_max_thinking_tokens`.
//!
//! [`answer_control_request`](crate::answer_control_request) hands the request over as a
//! [`DeferredControl`] or a [`PendingControl`]; [`AsyncControl`] starts the host call right away (so a duplicate is
//! recognised and a host callback is invoked at the moment the request arrives, as the upstream does) and returns the
//! task that writes the reply once the host settles. The caller awaits or spawns that task.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::CancellationToken;
use crate::bridge_worker_control::{
    CallbackResult, DeferredControl, HostError, HostFuture, ModelVerdict, PendingControl, Replier, clean_control_text,
};
use crate::session_prompt::js_truthy;

mod constants;

pub use constants::SIDE_QUESTION_DEADLINE;
use constants::{BACKGROUND_TASKS_DISABLED, MODEL_CODES, SIDE_QUESTION_NO_ANSWER, SIDE_QUESTION_TIMEOUT};

/// Where a frame goes: the transport's write.
pub type FrameWriter = Box<dyn Fn(Value) + Send + Sync>;
/// The id a frame carries (`iu`).
pub type UuidSource = Box<dyn Fn() -> String + Send + Sync>;
/// What writes a reply once the host settles.
pub type ControlTask = Pin<Box<dyn Future<Output = ()> + Send>>;
/// `P4`: whether background tasks are switched off for this session.
pub type BackgroundTasksSwitch = Box<dyn Fn() -> bool + Send + Sync>;
/// `onStopTask`: what the host resolves with, nothing when it resolves with nothing.
pub type StopTaskCallback = Box<dyn Fn(&str) -> HostFuture<Option<Value>> + Send + Sync>;
/// `onBackgroundTasks`: what became of the tool call (`None` for nothing).
pub type BackgroundTasksCallback = Box<dyn Fn(Option<&str>) -> HostFuture<Option<Value>> + Send + Sync>;
/// `onSideQuestion`: the parsed question and what the host needs to stop or report; the answer is an object, and
/// nothing counts as a failure like the upstream (a non-object answer cannot be handed over).
pub type SideQuestionCallback =
    Box<dyn Fn(&Value, SideQuestionContext) -> HostFuture<Option<Map<String, Value>>> + Send + Sync>;

/// The callbacks of the attach options that return a promise.
#[derive(Default)]
pub struct AsyncControlCallbacks {
    pub on_stop_task: Option<StopTaskCallback>,
    pub on_background_tasks: Option<BackgroundTasksCallback>,
    pub on_side_question: Option<SideQuestionCallback>,
}

struct Shared {
    session_id: String,
    write: FrameWriter,
    uuid: UuidSource,
    cancellable: Mutex<HashMap<String, CancellationToken>>,
}

impl Shared {
    fn running(&self) -> MutexGuard<'_, HashMap<String, CancellationToken>> {
        self.cancellable.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn replier<'a>(&'a self, id: Option<&'a Value>) -> Replier<'a> {
        Replier {
            session_id: &self.session_id,
            id,
        }
    }
}

/// The map key of a request id: the ids a client sends are strings or numbers, which print distinctly.
fn id_key(id: Option<&Value>) -> String {
    id.map_or_else(|| "undefined".to_owned(), Value::to_string)
}

/// Removes a request from the cancellable set when its task ends or is dropped.
struct Registration {
    shared: Arc<Shared>,
    key: String,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.shared.running().remove(&self.key);
    }
}

/// What the host of a side question uses while the question is open.
pub struct SideQuestionContext {
    /// Aborted when the client cancels the request or the deadline passes.
    pub signal: CancellationToken,
    shared: Arc<Shared>,
    request_id: Option<Value>,
}

impl SideQuestionContext {
    /// Reports progress: a `control_request_progress` frame that names the request and carries `update`. The keys of
    /// `update` keep their place and may replace the frame's own, except the id and the session, which are written
    /// last.
    pub fn report_progress(&self, update: Map<String, Value>) {
        let mut frame = Map::new();
        frame.insert("type".to_owned(), Value::from("system"));
        frame.insert("subtype".to_owned(), Value::from("control_request_progress"));
        if let Some(id) = &self.request_id {
            frame.insert("request_id".to_owned(), id.clone());
        }
        frame.extend(update);
        frame.insert("uuid".to_owned(), Value::String((self.shared.uuid)()));
        frame.insert("session_id".to_owned(), Value::String(self.shared.session_id.clone()));
        (self.shared.write)(Value::Object(frame));
    }
}

/// Starts the host calls of the control requests that wait for a promise and writes their replies.
pub struct AsyncControl {
    shared: Arc<Shared>,
    callbacks: AsyncControlCallbacks,
    background_tasks_disabled: BackgroundTasksSwitch,
}

impl AsyncControl {
    #[must_use]
    pub fn new(
        session_id: &str,
        write: FrameWriter,
        uuid: UuidSource,
        callbacks: AsyncControlCallbacks,
        background_tasks_disabled: BackgroundTasksSwitch,
    ) -> Self {
        Self {
            shared: Arc::new(Shared {
                session_id: session_id.to_owned(),
                write,
                uuid,
                cancellable: Mutex::new(HashMap::new()),
            }),
            callbacks,
            background_tasks_disabled,
        }
    }

    /// Cancels the side question the client named, if it is still open.
    pub fn cancel(&self, request_id: &Value) {
        if let Some(token) = self.shared.running().get(&id_key(Some(request_id))) {
            token.cancel();
        }
    }

    /// Aborts every request still open and forgets them (the handle's `close`).
    pub fn cancel_all(&self) {
        let mut running = self.shared.running();
        for token in running.values() {
            token.cancel();
        }
        running.clear();
    }

    /// Starts the host call of a deferred request. `None` means nothing is left to run: a side question whose id is
    /// already open is dropped without a reply.
    #[must_use]
    pub fn start(&self, request: &Value, work: DeferredControl) -> Option<ControlTask> {
        let id = request.get("request_id").cloned();
        match work {
            DeferredControl::StopTask { task_id } => {
                let host = self.callbacks.on_stop_task.as_ref()?(&task_id);
                Some(self.respond("stop_task", id, async move { host.await.map(object_or_empty) }))
            }
            DeferredControl::BackgroundTasks { tool_use_id } => {
                if (self.background_tasks_disabled)() {
                    let shared = Arc::clone(&self.shared);
                    return Some(Box::pin(async move {
                        let frame = shared
                            .replier(id.as_ref())
                            .frame_value("error", vec![("error", Value::from(BACKGROUND_TASKS_DISABLED))]);
                        (shared.write)(frame);
                    }));
                }
                let host = self.callbacks.on_background_tasks.as_ref()?(tool_use_id.as_deref());
                Some(self.respond("background_tasks", id, async move {
                    host.await.map(|answer| match (tool_use_id, answer) {
                        (Some(_), Some(backgrounded)) => {
                            let mut reply = Map::new();
                            reply.insert("backgrounded".to_owned(), backgrounded);
                            Value::Object(reply)
                        }
                        _ => Value::Object(Map::new()),
                    })
                }))
            }
            DeferredControl::SideQuestion { request: body } => self.side_question(id, &body),
        }
    }

    fn side_question(&self, id: Option<Value>, body: &Value) -> Option<ControlTask> {
        let key = id_key(id.as_ref());
        let token = CancellationToken::new();
        {
            let mut running = self.shared.running();
            if running.contains_key(&key) {
                tracing::warn!(request_id = %key, "side_question request id is already open; the duplicate is dropped");
                return None;
            }
            running.insert(key.clone(), token.clone());
        }
        let registration = Registration {
            shared: Arc::clone(&self.shared),
            key,
        };
        let context = SideQuestionContext {
            signal: token.clone(),
            shared: Arc::clone(&self.shared),
            request_id: id.clone(),
        };
        let host = self.callbacks.on_side_question.as_ref()?(&side_question_data(body), context);
        Some(self.respond("side_question", id, async move {
            let outcome = match tokio::time::timeout(SIDE_QUESTION_DEADLINE, host).await {
                Ok(answer) => answer.and_then(side_answer),
                Err(_) => {
                    token.cancel();
                    Err(HostError::Aborted(SIDE_QUESTION_TIMEOUT.to_owned()))
                }
            };
            drop(registration);
            outcome.map(Value::Object)
        }))
    }

    /// `Or`: writes the success reply with what the host resolved, or the error reply for what it rejected with.
    fn respond(
        &self,
        subtype: &'static str,
        id: Option<Value>,
        host: impl Future<Output = Result<Value, HostError>> + Send + 'static,
    ) -> ControlTask {
        let shared = Arc::clone(&self.shared);
        Box::pin(async move {
            let replier = shared.replier(id.as_ref());
            let frame = match host.await {
                Ok(response) => replier.frame_value("success", vec![("response", response)]),
                Err(error) => {
                    replier.frame_value("error", vec![("error", Value::String(failure_text(subtype, &error)))])
                }
            };
            (shared.write)(frame);
        })
    }

    /// Writes the reply to a `set_model` or `set_max_thinking_tokens` request once its promise settles.
    #[must_use]
    pub fn finish_pending(&self, request: &Value, pending: PendingControl) -> ControlTask {
        let shared = Arc::clone(&self.shared);
        let id = request.get("request_id").cloned();
        Box::pin(async move {
            match pending {
                PendingControl::SetModel(host) => finish_model(&shared, id.as_ref(), host.await),
                PendingControl::SetMaxThinkingTokens(host) => finish_thinking(&shared, id.as_ref(), host.await),
            }
        })
    }
}

/// `P4` without the settings (the attach path has none): background tasks are off when
/// `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS` or `CLAUDE_CODE_SIMPLE` reads as on, or `--bare` is among the arguments
/// before any `--`. `variable` reads the environment and is asked on every request.
#[must_use]
pub fn background_tasks_disabled(variable: impl Fn(&str) -> Option<String>, arguments: &[String]) -> bool {
    let on = |name: &str| variable(name).is_some_and(|value| js_truthy(&value));
    on("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS")
        || on("CLAUDE_CODE_SIMPLE")
        || arguments
            .iter()
            .take_while(|argument| *argument != "--")
            .any(|argument| argument == "--bare")
}

/// `Or`: a host that resolves with nothing answers with an empty object.
fn object_or_empty(answer: Option<Value>) -> Value {
    match answer {
        None | Some(Value::Null) => Value::Object(Map::new()),
        Some(answer) => answer,
    }
}

/// `Q4`: what the client is told when the host rejected. An abort says why (cleaned); anything else only names the
/// request.
fn failure_text(subtype: &str, error: &HostError) -> String {
    match error {
        HostError::Aborted(message) => clean_control_text(message),
        HostError::Failed(_) => {
            tracing::error!(subtype, error = %error.message(), "control request failed");
            format!("{subtype} failed")
        }
    }
}

/// What the host of a side question is handed: the question and history the request names, without anything else.
fn side_question_data(body: &Value) -> Value {
    let mut data = Map::new();
    data.insert(
        "question".to_owned(),
        body.get("question").cloned().unwrap_or(Value::Null),
    );
    if let Some(history) = body.get("history").and_then(Value::as_array) {
        let turns = history
            .iter()
            .map(|turn| {
                let mut kept = Map::new();
                for key in ["question", "response", "fallback_notice"] {
                    if let Some(value) = turn.get(key) {
                        kept.insert(key.to_owned(), value.clone());
                    }
                }
                Value::Object(kept)
            })
            .collect();
        data.insert("history".to_owned(), Value::Array(turns));
    }
    Value::Object(data)
}

/// `GPe`: an answer that is not marked as real is replaced by the standard text, unless it is null.
fn side_answer(answer: Option<Map<String, Value>>) -> Result<Map<String, Value>, HostError> {
    let Some(mut answer) = answer else {
        return Err(HostError::Failed(
            "the host answered a side question with nothing".to_owned(),
        ));
    };
    let answered = answer.get("response").is_none_or(|response| !response.is_null());
    if answered && answer.get("synthetic") != Some(&Value::Bool(false)) {
        answer.insert("response".to_owned(), Value::from(SIDE_QUESTION_NO_ANSWER));
        answer.insert("synthetic".to_owned(), Value::Bool(true));
    }
    Ok(answer)
}

/// The `informational` system frame that carries a `set_model` notice.
#[derive(Serialize)]
struct InformationalFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    subtype: &'static str,
    content: String,
    level: &'static str,
    uuid: String,
    session_id: &'a str,
}

fn write_reply(shared: &Shared, id: Option<&Value>, subtype: &str, tail: Vec<(&str, Value)>) {
    let mut response = Map::new();
    response.insert("subtype".to_owned(), Value::from(subtype));
    response.extend(tail.into_iter().map(|(key, value)| (key.to_owned(), value)));
    if let Some(id) = id {
        response.insert("request_id".to_owned(), id.clone());
    }
    let mut frame = Map::new();
    frame.insert("type".to_owned(), Value::from("control_response"));
    frame.insert("response".to_owned(), Value::Object(response));
    frame.insert("session_id".to_owned(), Value::String(shared.session_id.clone()));
    (shared.write)(Value::Object(frame));
}

/// `tF`: the reply to a `set_model` verdict. The request id comes last, after the verdict's own fields.
fn finish_model(shared: &Shared, id: Option<&Value>, outcome: Result<Option<ModelVerdict>, HostError>) {
    let verdict = match outcome {
        Ok(verdict) => verdict.unwrap_or(ModelVerdict {
            ok: true,
            ..ModelVerdict::default()
        }),
        Err(error) => {
            tracing::error!(error = %error.message(), "set_model failed");
            write_reply(
                shared,
                id,
                "error",
                vec![
                    ("error", Value::from("set_model failed")),
                    ("error_code", Value::from("internal_error")),
                ],
            );
            return;
        }
    };
    let model = verdict.model.as_deref().map(clean_control_text);
    if verdict.ok {
        for notice in verdict.notices.iter().flatten() {
            let frame = InformationalFrame {
                kind: "system",
                subtype: "informational",
                content: clean_control_text(notice),
                level: "notice",
                uuid: (shared.uuid)(),
                session_id: &shared.session_id,
            };
            match serde_json::to_value(&frame) {
                Ok(frame) => (shared.write)(frame),
                Err(error) => {
                    write_reply(shared, id, "error", vec![("error", Value::String(error.to_string()))]);
                    return;
                }
            }
        }
        let tail = model
            .map(|model| {
                let mut reply = Map::new();
                reply.insert("model".to_owned(), Value::String(model));
                ("response", Value::Object(reply))
            })
            .into_iter()
            .collect();
        write_reply(shared, id, "success", tail);
        return;
    }
    let error = verdict.error.as_deref().map_or_else(String::new, |text| {
        text.split('\n').map(clean_control_text).collect::<Vec<_>>().join("\n")
    });
    let mut tail = vec![("error", Value::String(error))];
    if let Some(code) = verdict.code.as_deref().filter(|code| MODEL_CODES.contains(code)) {
        tail.push(("error_code", Value::from(code)));
    }
    if let Some(model) = model {
        tail.push(("model", Value::String(model)));
    }
    write_reply(shared, id, "error", tail);
}

/// `Z4`: the reply to a `set_max_thinking_tokens` verdict, or to its rejection.
fn finish_thinking(shared: &Shared, id: Option<&Value>, outcome: Result<Option<CallbackResult>, HostError>) {
    let replier = shared.replier(id);
    let frame = match outcome {
        Ok(Some(CallbackResult::Failed { error, .. })) => replier.frame_value(
            "error",
            vec![(
                "error",
                Value::String(clean_control_text(error.as_deref().unwrap_or_default())),
            )],
        ),
        Ok(_) => replier.frame_value("success", Vec::new()),
        Err(error) => replier.frame_value(
            "error",
            vec![("error", Value::String(failure_text("set_max_thinking_tokens", &error)))],
        ),
    };
    (shared.write)(frame);
}
