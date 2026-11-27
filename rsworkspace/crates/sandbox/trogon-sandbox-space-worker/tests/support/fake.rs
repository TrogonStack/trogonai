//! An in-process SPACE: a control plane and a data plane served by one mock server.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use wiremock::matchers::any;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

pub const MANAGEMENT_TOKEN: &str = "management-secret";
pub const ATTACH_TOKEN: &str = "attach-secret";

/// One answer to a request for a command's event stream.
#[derive(Clone)]
pub enum Reply {
    Body(String),
    Status(u16),
    Stall,
}

/// What SPACE does with the next command created.
#[derive(Clone)]
pub enum Plan {
    Events(Vec<Reply>),
    NoAttach,
    NoEventsUrl,
    CreateStatus(u16),
    EventsAt(String),
}

pub struct Sandbox {
    pub state: String,
    pub metadata: Value,
    pub failure_code: Option<String>,
}

#[derive(Default)]
pub struct State {
    pub sandboxes: BTreeMap<String, Sandbox>,
    pub operations: HashMap<String, (String, u32)>,
    pub counter: u32,
    pub log: Vec<String>,
    pub create_status: Option<u16>,
    pub create_without_id: bool,
    pub create_state: Option<String>,
    pub create_delay: Duration,
    pub embed_terminal_operation: bool,
    pub operation_pending_polls: u32,
    pub operation_final: Option<String>,
    pub boot_polls: u32,
    pub probe_malformed: bool,
    pub probe_status: Option<u16>,
    pub lifecycle_stuck: bool,
    pub lifecycle_delay: Duration,
    pub delete_conflicts: u32,
    pub delete_keeps: bool,
    pub delete_marks_deleted: bool,
    pub delete_status: Option<u16>,
    pub list_in_pages: bool,
    pub list_status: Option<u16>,
    pub kill_status: Option<u16>,
    pub next_plans: VecDeque<Plan>,
    pub commands: Vec<Value>,
    pub event_requests: Vec<(String, Option<String>, String)>,
    pub fs: HashMap<String, Vec<u8>>,
    pub modes: HashMap<String, String>,
    pub fail_script: Option<&'static str>,
    pub empty_mktemp: bool,
    pub garbage_read: bool,
    pub fs_exit: HashMap<String, i64>,
    pub create_bodies: Vec<Value>,
    pub replies: HashMap<String, (Vec<Reply>, usize)>,
}

pub struct Space {
    pub server: MockServer,
    state: Arc<Mutex<State>>,
}

struct Handler(Arc<Mutex<State>>);

pub fn frame(id: u32, data: &Value) -> String {
    format!("id: {id}\ndata: {data}\n\n")
}

fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn exit_body(stdout: &[u8], stderr: &[u8], code: i64, status: &str) -> String {
    let mut body = String::new();
    let mut id = 0;
    if !stdout.is_empty() {
        id += 1;
        body += &frame(id, &json!({"type": "stdout", "data_base64": encode(stdout)}));
    }
    if !stderr.is_empty() {
        id += 1;
        body += &frame(id, &json!({"type": "stderr", "data_base64": encode(stderr)}));
    }
    body + &frame(
        id + 1,
        &json!({"type": "process_exited", "exit_code": code, "status": status}),
    )
}

pub fn exit_plan(stdout: &[u8], code: i64) -> Plan {
    Plan::Events(vec![Reply::Body(exit_body(stdout, b"", code, "exited"))])
}

fn error(status: u16, code: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(json!({"error": {"code": code, "message": "scripted failure"}}))
}

fn ok(body: &Value) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(body)
}

impl State {
    fn next_id(&mut self, prefix: &str) -> String {
        self.counter += 1;
        format!("{prefix}-{}", self.counter)
    }

    fn operation(&mut self, final_state: &str) -> String {
        let id = self.next_id("op");
        self.operations
            .insert(id.clone(), (final_state.to_owned(), self.operation_pending_polls));
        id
    }

    fn create(&mut self, body: &Value) -> ResponseTemplate {
        self.create_bodies.push(body.clone());
        if let Some(status) = self.create_status {
            return error(status, "scripted");
        }
        let sandbox = self.next_id("sb");
        self.sandboxes.insert(
            sandbox.clone(),
            Sandbox {
                state: self.create_state.clone().unwrap_or_else(|| "running".into()),
                metadata: body["metadata"].clone(),
                failure_code: Some("boot_failed".into()),
            },
        );
        let final_state = self.operation_final.clone().unwrap_or_else(|| "succeeded".into());
        let operation = self.operation(&final_state);
        let mut reply = json!({"operation_id": operation, "sandbox_id": sandbox});
        if self.create_without_id {
            reply = json!({"operation_id": operation});
        }
        if self.embed_terminal_operation {
            reply["operation"] = json!({"state": "succeeded"});
        }
        ok(&reply).set_delay(self.create_delay)
    }

    fn sandbox_json(&mut self, id: &str) -> Option<Value> {
        let boot = self.boot_polls > 0;
        if boot {
            self.boot_polls -= 1;
        }
        let sandbox = self.sandboxes.get(id)?;
        Some(json!({
            "id": id,
            "state": if boot { "starting" } else { sandbox.state.as_str() },
            "failure_code": sandbox.failure_code,
            "metadata": sandbox.metadata,
        }))
    }

    fn list(&mut self, query: &str) -> ResponseTemplate {
        if let Some(status) = self.list_status {
            return error(status, "scripted");
        }
        let ids: Vec<String> = self.sandboxes.keys().cloned().collect();
        let (from, to, next) = if self.list_in_pages && !query.contains("cursor=") && ids.len() > 1 {
            (0, 1, Some("page-2"))
        } else if self.list_in_pages && query.contains("cursor=") {
            (1, ids.len(), None)
        } else {
            (0, ids.len(), None)
        };
        let data: Vec<Value> = ids[from..to]
            .iter()
            .filter_map(|id| {
                let sandbox = self.sandboxes.get(id)?;
                Some(json!({"id": id, "state": sandbox.state, "metadata": sandbox.metadata}))
            })
            .collect();
        ok(&json!({"data": data, "next_cursor": next}))
    }

    fn delete(&mut self, id: &str) -> ResponseTemplate {
        if let Some(status) = self.delete_status {
            return error(status, "scripted");
        }
        if self.delete_conflicts > 0 {
            self.delete_conflicts -= 1;
            return error(409, "conflict");
        }
        if !self.sandboxes.contains_key(id) {
            return error(404, "not_found");
        }
        if self.delete_marks_deleted {
            if let Some(sandbox) = self.sandboxes.get_mut(id) {
                sandbox.state = "deleted".into();
            }
        } else if !self.delete_keeps {
            self.sandboxes.remove(id);
        }
        let operation = self.operation("succeeded");
        ok(&json!({"operation_id": operation, "operation": {"state": "succeeded"}}))
    }

    fn lifecycle(&mut self, id: &str, verb: &str) -> ResponseTemplate {
        let Some(sandbox) = self.sandboxes.get_mut(id) else {
            return error(404, "not_found");
        };
        sandbox.state = if self.lifecycle_stuck {
            "pausing".into()
        } else {
            match verb {
                "pause" => "paused",
                "suspend" => "suspended",
                _ => "running",
            }
            .into()
        };
        let operation = self.operation("succeeded");
        ok(&json!({"operation_id": operation})).set_delay(self.lifecycle_delay)
    }

    fn operation_json(&mut self, id: &str) -> ResponseTemplate {
        let Some((state, polls)) = self.operations.get_mut(id) else {
            return error(404, "not_found");
        };
        if *polls > 0 {
            *polls -= 1;
            return ok(&json!({"id": id, "state": "running"}));
        }
        let state = state.clone();
        ok(&json!({"id": id, "state": state, "terminal_error_code": "boom", "terminal_error": {"message": "it broke"}}))
    }

    fn command(&mut self, id: &str, body: &Value) -> ResponseTemplate {
        self.commands.push(body.clone());
        if !self.sandboxes.contains_key(id) {
            return error(404, "not_found");
        }
        let plan = match self.next_plans.pop_front() {
            Some(plan) => plan,
            None => self.guest(body),
        };
        let process = self.next_id("proc");
        let events = format!("/events/{process}");
        let attach = match &plan {
            Plan::CreateStatus(status) => return error(*status, "scripted"),
            Plan::NoAttach => Value::Null,
            Plan::NoEventsUrl => json!({"token": ATTACH_TOKEN}),
            Plan::EventsAt(url) => json!({"events_url": url, "token": ATTACH_TOKEN}),
            Plan::Events(_) => json!({"events_url": events, "token": ATTACH_TOKEN}),
        };
        if let Plan::Events(replies) = plan {
            self.replies.insert(process.clone(), (replies, 0));
        }
        ok(&json!({"process_id": process, "attach": attach}))
    }

    fn guest(&mut self, body: &Value) -> Plan {
        let argv: Vec<String> = body["command"]
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        let Some(script) = argv
            .get(2)
            .filter(|_| argv.get(3).map(String::as_str) == Some("sandbox-files"))
        else {
            return exit_plan(argv.join(" ").as_bytes(), 0);
        };
        let args = &argv[4..];
        if self.fail_script.is_some_and(|needle| script.contains(needle)) {
            return exit_plan(b"", 1);
        }
        let decoded = |text: &str| {
            base64::engine::general_purpose::STANDARD
                .decode(text)
                .unwrap_or_default()
        };
        if script.contains("mktemp") {
            if self.empty_mktemp {
                return exit_plan(b"", 0);
            }
            let temporary = format!("{}.sandbox.tmp", args[0]);
            self.fs.insert(temporary.clone(), Vec::new());
            return exit_plan(format!("{temporary}\n").as_bytes(), 0);
        }
        if script.contains("base64 -d") {
            let chunk = decoded(&args[1]);
            self.fs.entry(args[0].clone()).or_default().extend(chunk);
            return exit_plan(b"", 0);
        }
        if script.contains("mv -f") {
            let data = self.fs.remove(&args[0]).unwrap_or_default();
            self.fs.insert(args[1].clone(), data);
            self.modes.insert(args[1].clone(), args[2].clone());
            return exit_plan(b"", 0);
        }
        if script.contains("rm -f") {
            self.fs.remove(&args[0]);
            return exit_plan(b"", 0);
        }
        if let Some(code) = self.fs_exit.get(&args[0]) {
            return exit_plan(b"", *code);
        }
        let Some(data) = self.fs.get(&args[0]) else {
            return exit_plan(b"", 3);
        };
        if self.garbage_read {
            return exit_plan(b"!!not base64!!", 0);
        }
        let index: usize = args[1].parse().unwrap_or(0);
        let from = (index * 524_288).min(data.len());
        let to = (from + 524_288).min(data.len());
        exit_plan(encode(&data[from..to]).as_bytes(), 0)
    }

    fn events(&mut self, process: &str, last_id: Option<String>, auth: String) -> ResponseTemplate {
        self.event_requests.push((process.to_owned(), last_id, auth));
        let Some((replies, served)) = self.replies.get_mut(process) else {
            return error(404, "not_found");
        };
        let at = (*served).min(replies.len() - 1);
        *served += 1;
        match &replies[at] {
            Reply::Body(body) => ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body.clone()),
            Reply::Status(status) => error(*status, "scripted"),
            Reply::Stall => ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("")
                .set_delay(Duration::from_secs(30)),
        }
    }
}

impl Respond for Handler {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let Ok(mut state) = self.0.lock() else {
            return ResponseTemplate::new(500);
        };
        let method = request.method.as_str().to_owned();
        let path = request.url.path().to_owned();
        let query = request.url.query().unwrap_or("").to_owned();
        state.log.push(format!("{method} {path}"));
        let body: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let header = |name: &str| {
            request
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned)
        };
        let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        match (method.as_str(), segments.as_slice()) {
            ("POST", ["v1", "sandboxes"]) => state.create(&body),
            ("GET", ["v1", "sandboxes"]) => state.list(&query),
            ("GET", ["v1", "operations", id]) => state.operation_json(id),
            ("GET", ["v1", "sandboxes", id]) => match state.sandbox_json(id) {
                Some(sandbox) => ok(&sandbox),
                None => error(404, "not_found"),
            },
            ("DELETE", ["v1", "sandboxes", id]) => state.delete(id),
            ("POST", ["v1", "sandboxes", target]) => match target.split_once(':') {
                Some((id, verb)) => state.lifecycle(id, verb),
                None => error(404, "not_found"),
            },
            ("GET", ["v1", "sandboxes", id, "processes"]) => {
                if let Some(status) = state.probe_status {
                    error(status, "scripted")
                } else if state.probe_malformed {
                    ok(&json!({"data": "nope"}))
                } else if state.sandboxes.contains_key(*id) {
                    ok(&json!({"data": []}))
                } else {
                    error(404, "not_found")
                }
            }
            ("POST", ["v1", "sandboxes", id, "commands"]) => state.command(id, &body),
            ("POST", ["v1", "sandboxes", _, "processes", _]) => match state.kill_status {
                Some(status) => error(status, "scripted"),
                None => ok(&json!({})),
            },
            ("GET", ["events", process]) => state.events(
                process,
                header("last-event-id"),
                header("authorization").unwrap_or_default(),
            ),
            _ => ResponseTemplate::new(404),
        }
    }
}

impl Space {
    pub async fn start() -> Self {
        let server = MockServer::start().await;
        let state = Arc::new(Mutex::new(State::default()));
        Mock::given(any())
            .respond_with(Handler(Arc::clone(&state)))
            .mount(&server)
            .await;
        Self { server, state }
    }

    pub fn uri(&self) -> String {
        self.server.uri()
    }

    /// The scripted state, for setting up faults and reading what SPACE saw.
    pub fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
