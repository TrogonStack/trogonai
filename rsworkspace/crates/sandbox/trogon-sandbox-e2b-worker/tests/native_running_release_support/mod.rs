// Used by one test binary, which exercises only part of it.
#![allow(dead_code, reason = "shared test support; not every item is used by every binary")]

//! A proxy between the worker and the fake native envd that holds the one authenticated Release request of a
//! command launch until the test decides what becomes of it.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{Request, State};
use axum::http::request::Parts;
use axum::http::{Method, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use serde_json::{Value, json};
use tokio::sync::{oneshot, watch};
use trogon_sandbox_contract::{
    DispatchAttemptId, LeaseEpoch, ProviderKind, RunCommandRequest, RunCommandResponse, WorkerId,
};
use trogon_sandbox_e2b_worker::{E2bConfig, E2bExec, Envd, EnvdTarget, LeaseSandbox};
use trogon_sandbox_protocol::{DispatchSigner, DispatchVerifier, EnforcementMode, EpochMode, VerbScope, subjects};
use url::Url;

use crate::ensure_support::{Boxed, Context};
use crate::native_command_support::Binding;
use crate::worker_process_support::Nats;

/// The script the worker runs natively to release a prepared launch; it must match the worker's own.
const RELEASE_SCRIPT: &str = r#"d=$1; p=$2; expected=$3
IFS= read -r line <"/proc/$p/stat" || exit 1
fields=${line##*) }; set -- $fields
[ "$3" = "$p" ] && [ "$4" = "$p" ] || exit 1
shift 19
[ "$1" = "$expected" ] || exit 1
: >"$d/ready"
"#;

const MAX_BODY: usize = 128 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    Hold,
    Forward,
    Discard,
}

/// What became of the held request once the test let it go.
#[derive(Clone, Copy)]
pub struct Outcome {
    pub forwarded: bool,
    pub native_exit: Option<i32>,
}

struct Control {
    upstream: String,
    client: reqwest::Client,
    armed: AtomicBool,
    reached: watch::Sender<Option<Binding>>,
    action: watch::Receiver<Action>,
    completed: watch::Sender<Option<Outcome>>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
    selected: AtomicUsize,
}

/// The connect frames of a body: a flag byte, a big-endian length, and that many bytes.
fn frames(mut body: &[u8]) -> Vec<(u8, &[u8])> {
    let mut frames = Vec::new();
    while let Some((&[flag, a, b, c, d], rest)) = body.split_first_chunk::<5>()
        && let Some((payload, rest)) = rest.split_at_checked(u32::from_be_bytes([a, b, c, d]) as usize)
    {
        frames.push((flag, payload));
        body = rest;
    }
    frames
}

/// The native process a Release request names, when `body` is exactly that request.
fn release_binding(body: &[u8]) -> Option<Binding> {
    let all = frames(body);
    let [(0, payload)] = all.as_slice() else {
        return None;
    };
    let value: Value = serde_json::from_slice(payload).ok()?;
    let args = value["process"]["args"].as_array()?;
    let envelope = args.iter().take(8).map(Value::as_str).collect::<Vec<_>>()
        == [
            Some("-l"),
            Some("-c"),
            Some("exec \"$@\""),
            Some("bash"),
            Some("bash"),
            Some("-c"),
            Some(RELEASE_SCRIPT),
            Some("bash"),
        ];
    (envelope && args.len() == 11 && value["process"]["cmd"] == "/bin/bash").then_some(())?;
    Some(Binding {
        pid: args.get(9)?.as_str()?.parse().ok()?,
        started: args.get(10)?.as_str()?.parse().ok()?,
    })
}

/// The exit code of the native process a response reports as ended.
fn native_end(body: &[u8]) -> Option<i32> {
    frames(body)
        .into_iter()
        .filter(|(flag, _)| *flag == 0)
        .find_map(|(_, payload)| {
            serde_json::from_slice::<Value>(payload)
                .ok()?
                .pointer("/event/end")
                .cloned()
        })
        .filter(|end| end["exited"] == true)
        .and_then(|end| end.get("exitCode").and_then(Value::as_i64).unwrap_or(0).try_into().ok())
}

async fn upstream(control: &Control, mut parts: Parts, body: Bytes) -> Result<(Response, Option<i32>), StatusCode> {
    parts.headers.remove(header::HOST);
    let mut response = control
        .client
        .request(parts.method, format!("{}{}", control.upstream, parts.uri))
        .headers(parts.headers)
        .body(body)
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    let status = response.status();
    let mut headers = response.headers().clone();
    headers.remove(header::TRANSFER_ENCODING);
    headers.remove(header::CONTENT_LENGTH);
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| StatusCode::BAD_GATEWAY)? {
        body.extend_from_slice(&chunk);
    }
    let exit = status.is_success().then(|| native_end(&body)).flatten();
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok((response, exit))
}

async fn forward(State(control): State<Arc<Control>>, request: Request) -> Result<Response, StatusCode> {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, MAX_BODY).await.map_err(|_| StatusCode::BAD_REQUEST)?;
    let selected = (parts.method == Method::POST && parts.uri.path() == "/process.Process/Start")
        .then(|| release_binding(&body))
        .flatten()
        .filter(|_| control.armed.swap(false, Ordering::SeqCst));
    let Some(binding) = selected else {
        return upstream(&control, parts, body).await.map(|(response, _)| response);
    };
    control.selected.fetch_add(1, Ordering::SeqCst);
    let (send, response) = oneshot::channel();
    let owned = Arc::clone(&control);
    let mut action = control.action.clone();
    let task = tokio::spawn(async move {
        let chosen = tokio::time::timeout(
            Duration::from_secs(30),
            action.wait_for(|action| *action != Action::Hold),
        )
        .await
        .ok()
        .and_then(Result::ok)
        .map_or(Action::Discard, |value| *value);
        let result = match chosen {
            Action::Forward => upstream(&owned, parts, body).await,
            Action::Hold | Action::Discard => Err(StatusCode::GONE),
        };
        owned.completed.send_replace(Some(Outcome {
            forwarded: chosen == Action::Forward,
            native_exit: result.as_ref().ok().and_then(|(_, exit)| *exit),
        }));
        let _ = send.send(result.map(|(response, _)| response));
    });
    *control.task.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? = Some(task);
    control.reached.send_replace(Some(binding));
    response.await.map_err(|_| StatusCode::BAD_GATEWAY)?
}

pub struct HeldRelease {
    pub url: String,
    control: Arc<Control>,
    reached: watch::Receiver<Option<Binding>>,
    action: watch::Sender<Action>,
    completed: watch::Receiver<Option<Outcome>>,
    server: Option<tokio::task::JoinHandle<()>>,
}

impl HeldRelease {
    pub async fn start(upstream: &str) -> Result<Self, Boxed> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let (reached_tx, reached) = watch::channel(None);
        let (action, action_rx) = watch::channel(Action::Hold);
        let (completed_tx, completed) = watch::channel(None);
        let control = Arc::new(Control {
            upstream: upstream.trim_end_matches('/').into(),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .build()?,
            armed: AtomicBool::new(false),
            reached: reached_tx,
            action: action_rx,
            completed: completed_tx,
            task: Mutex::new(None),
            selected: AtomicUsize::new(0),
        });
        let app = axum::Router::new().fallback(forward).with_state(Arc::clone(&control));
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            url,
            control,
            reached,
            action,
            completed,
            server: Some(server),
        })
    }

    /// Selects the next Release request for holding.
    pub fn arm(&self) {
        self.control.armed.store(true, Ordering::SeqCst);
    }

    /// Whether the held request is still waiting for its fate.
    pub fn pending(&self) -> bool {
        self.control.selected.load(Ordering::SeqCst) == 1
            && self.control.completed.borrow().is_none()
            && self
                .control
                .task
                .lock()
                .is_ok_and(|task| task.as_ref().is_some_and(|task| !task.is_finished()))
    }

    /// Waits for the armed Release request to arrive, and names the native process it addresses.
    pub async fn reached(&mut self) -> Result<Binding, Boxed> {
        let binding = tokio::time::timeout(Duration::from_secs(8), self.reached.wait_for(Option::is_some))
            .await
            .context("the native release did not reach its hold")??;
        (*binding).context("the held release named no process")
    }

    /// Sends the held request on to the native envd and reports what happened.
    pub async fn forward_late(&mut self) -> Result<Outcome, Boxed> {
        (self.control.selected.load(Ordering::SeqCst) == 1)
            .then_some(())
            .context("exactly one authenticated release must be held")?;
        self.action.send_replace(Action::Forward);
        let outcome = tokio::time::timeout(Duration::from_secs(8), self.completed.wait_for(Option::is_some))
            .await
            .context("the held release did not complete its native attempt")??;
        (*outcome).context("the held release recorded no outcome")
    }

    pub async fn shutdown(mut self) -> Result<(), Boxed> {
        self.action.send_replace(Action::Discard);
        let task = self
            .control
            .task
            .lock()
            .map_err(|_| "the held task is poisoned")?
            .take();
        if let Some(task) = task {
            tokio::time::timeout(Duration::from_secs(8), task)
                .await
                .context("the held native release task did not settle")??;
        }
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
        Ok(())
    }
}

impl Drop for HeldRelease {
    fn drop(&mut self) {
        self.action.send_replace(Action::Discard);
        if let Some(task) = self.control.task.lock().ok().and_then(|mut task| task.take()) {
            task.abort();
        }
        if let Some(server) = &self.server {
            server.abort();
        }
    }
}

/// A native process reply: one data event, the end event and the trailer.
fn completed(stdout: &str) -> Vec<u8> {
    let stdout = base64::engine::general_purpose::STANDARD.encode(stdout);
    [
        (0, json!({"event":{"data":{"stdout":stdout}}})),
        (0, json!({"event":{"end":{"exited":true}}})),
        (2, json!({})),
    ]
    .into_iter()
    .flat_map(|(flag, event): (u8, Value)| {
        let body = event.to_string().into_bytes();
        [vec![flag], (body.len() as u32).to_be_bytes().to_vec(), body].concat()
    })
    .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actual_public_exec_release_serialization_is_recognized_by_the_hold_classifier() -> Result<(), Boxed> {
    let nats = Nats::start().await;
    let captured = Arc::new(Mutex::new(Vec::new()));
    let received = Arc::clone(&captured);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let endpoint = Url::parse(&format!("http://{}", listener.local_addr()?))?;
    let app = axum::Router::new().fallback(move |request: Request| {
        let captured = Arc::clone(&received);
        async move {
            (request.method() == Method::POST && request.uri().path() == "/process.Process/Start")
                .then_some(())
                .ok_or(StatusCode::NOT_FOUND)?;
            let body = to_bytes(request.into_body(), MAX_BODY)
                .await
                .map_err(|_| StatusCode::BAD_REQUEST)?;
            let value: Value = serde_json::from_slice(body.get(5..).ok_or(StatusCode::BAD_REQUEST)?)
                .map_err(|_| StatusCode::BAD_REQUEST)?;
            let script = value
                .pointer("/process/args/6")
                .and_then(Value::as_str)
                .ok_or(StatusCode::BAD_REQUEST)?;
            let release = script == RELEASE_SCRIPT;
            let stdout = (release || (script.contains("runner-ready") && script.contains("nohup setsid")))
                .then_some(if release { "" } else { "123 456\n" })
                .ok_or(StatusCode::NOT_FOUND)?;
            captured
                .lock()
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
                .extend(release.then(|| body.to_vec()));
            Ok::<_, StatusCode>(Response::new(Body::from(completed(stdout))))
        }
    });
    let server = tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let exercised = async {
        let config = E2bConfig::new(endpoint.clone(), "e2b.test", "serialization-fixture")?.with_envd_url(endpoint)?;
        let sandbox = LeaseSandbox {
            sandbox_id: "serialization-owned-native".into(),
            target: EnvdTarget::for_sandbox(&config, "serialization-owned-native", None, None)?,
            traffic_token: None,
            domain: "e2b.test".into(),
            exposed_ports: vec![],
        };
        let exec = E2bExec::new(Envd::new(Duration::from_secs(2))?);
        exec.configure_command_launches(async_nats::jetstream::new(nats.client.clone()))?;
        let request: RunCommandRequest = serde_json::from_value(json!({
            "lease_id":"serialization-lease","launch_key":"serialization-launch",
            "require_launch_cancellation":true,"command":["true"],"env":BTreeMap::<String,String>::new(),
            "detached":true,"deadline":"lease","run_as_user":"root",
        }))?;
        let signer = DispatchSigner::new("serialization-fixture", [17; 32]);
        let verifier = DispatchVerifier::new(EnforcementMode::Require)
            .with_epoch_mode(EpochMode::Require)
            .with_key(signer.key_id(), &signer.public_key_b64())?;
        let scope = VerbScope {
            tenant: crate::authority_support::lease_request()?.tenant,
            lease_id: request.lease_id.clone(),
            worker_id: Some(WorkerId::new("serialization-worker")?),
            attempt_id: Some(DispatchAttemptId::new("serialization-attempt")?),
            epoch: Some(LeaseEpoch::new(7)?),
        };
        let subject = subjects::provider_run_command(ProviderKind::E2b, &request.lease_id);
        let payload = serde_json::to_vec_pretty(&request)?;
        let signature = signer.sign_verb(&subject, &payload, &scope)?;
        let verified = verifier.verify_verb_scope(&subject, &signature, &payload, false, SystemTime::now())?;
        let response = tokio::time::timeout(
            Duration::from_secs(5),
            exec.handle_run_command_with_scope(&sandbox, &request, &verified, &payload),
        )
        .await?;
        (matches!(response, RunCommandResponse::Started(_)))
            .then_some(())
            .context("the public exec serializer must complete its local captured launch")
    }
    .await;
    server.abort();
    let _ = server.await;
    exercised?;
    let captures = captured.lock().map_err(|_| "the capture record is poisoned")?;
    assert_eq!(
        captures.len(),
        1,
        "the real public exec must emit exactly one native Release request"
    );
    let capture = captures.first().context("no Release request was captured")?;
    let value: Value = serde_json::from_slice(capture.get(5..).context("the Release request has no frame")?)?;
    let args = value["process"]["args"].as_array().context("missing serialized argv")?;
    assert!(
        value["process"]["cmd"] == "/bin/bash"
            && args.len() == 11
            && args[6] == RELEASE_SCRIPT
            && args[9] == "123"
            && args[10] == "456",
        "the public exec must use the production login-shell serialization envelope"
    );
    assert!(
        release_binding(capture).is_some_and(|binding| binding.pid == 123 && binding.started == 456),
        "the release hold classifier missed the actual public serialization envelope"
    );
    Ok(())
}
