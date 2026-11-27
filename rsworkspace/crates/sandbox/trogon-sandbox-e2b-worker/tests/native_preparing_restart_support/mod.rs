// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

//! A proxy in front of the native envd that holds the response of one selected bootstrap request, so a test can end
//! the worker while the native birth exists but the worker has not yet recorded it.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use serde_json::{Value, json};
use tokio::sync::{oneshot, watch};

use crate::ensure_support::{Boxed, Context};
use crate::native_command_support::Binding;

struct Control {
    upstream: String,
    client: reqwest::Client,
    armed: AtomicBool,
    reached: watch::Sender<Option<Binding>>,
    discard: watch::Receiver<bool>,
    selected: AtomicUsize,
    completed: AtomicBool,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Control {
    fn task(&self) -> std::sync::MutexGuard<'_, Option<tokio::task::JoinHandle<()>>> {
        self.task.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The deferred bootstrap request: the login shell running the custody script in a fresh execution directory.
fn selected_bootstrap(body: &[u8]) -> bool {
    let Some((header, frame)) = body.split_at_checked(5) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<Value>(frame) else {
        return false;
    };
    let Some(args) = value["process"]["args"].as_array() else {
        return false;
    };
    header[0] == 0
        && value["process"]["cmd"] == "/bin/bash"
        && args.len() >= 14
        && Value::Array(args[..6].to_vec()) == json!(["-l", "-c", r#"exec "$@""#, "bash", "bash", "-c"])
        && args[7] == "bash"
        && args[12] == "deferred"
        && args[8]
            .as_str()
            .is_some_and(|path| path.starts_with("/tmp/.trogon-exec-"))
}

/// The process group the bootstrap reported on its standard output as `pid started`.
fn native_binding(body: &[u8]) -> Option<Binding> {
    let mut stdout = Vec::new();
    let mut remaining = body;
    while let Some((header, rest)) = remaining.split_at_checked(5) {
        let length = u32::from_be_bytes(<[u8; 4]>::try_from(&header[1..]).ok()?) as usize;
        let (frame, rest) = rest.split_at_checked(length)?;
        let value: Value = serde_json::from_slice(frame).ok()?;
        if let Some(data) = value.pointer("/event/data/stdout").and_then(Value::as_str) {
            stdout.extend(base64::engine::general_purpose::STANDARD.decode(data).ok()?);
        }
        remaining = rest;
    }
    let text = String::from_utf8(stdout).ok()?;
    let (pid, started) = text.trim().split_once(' ')?;
    Some(Binding {
        pid: pid.parse().ok()?,
        started: started.parse().ok()?,
    })
}

async fn upstream(
    control: &Control,
    mut parts: axum::http::request::Parts,
    body: axum::body::Bytes,
) -> Result<(Response, Option<Binding>), StatusCode> {
    parts.headers.remove(header::HOST);
    let response = control
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
    let bytes = response.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    let binding = native_binding(&bytes);
    let mut response = Response::new(Body::from(bytes));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok((response, binding))
}

async fn forward(State(control): State<Arc<Control>>, request: Request) -> Result<Response, StatusCode> {
    let (parts, body) = request.into_parts();
    let body = to_bytes(body, 128 * 1024).await.map_err(|_| StatusCode::BAD_REQUEST)?;
    let selected = parts.method == Method::POST
        && parts.uri.path() == "/process.Process/Start"
        && selected_bootstrap(&body)
        && control.armed.swap(false, Ordering::SeqCst);
    if !selected {
        return upstream(&control, parts, body).await.map(|(response, _)| response);
    }
    control.selected.fetch_add(1, Ordering::SeqCst);
    let (send, response) = oneshot::channel();
    let owned = Arc::clone(&control);
    let mut discard = control.discard.clone();
    let task = tokio::spawn(async move {
        if let Ok((_, Some(binding))) = upstream(&owned, parts, body).await {
            owned.reached.send_replace(Some(binding));
        }
        let _ = tokio::time::timeout(Duration::from_secs(30), discard.wait_for(|discard| *discard)).await;
        owned.completed.store(true, Ordering::SeqCst);
        let _ = send.send(Err(StatusCode::GONE));
    });
    *control.task() = Some(task);
    response.await.map_err(|_| StatusCode::BAD_GATEWAY)?
}

pub struct HeldBootstrapReply {
    pub url: String,
    control: Arc<Control>,
    reached: watch::Receiver<Option<Binding>>,
    discard: watch::Sender<bool>,
    server: Option<tokio::task::JoinHandle<()>>,
}

impl HeldBootstrapReply {
    pub async fn start(upstream: &str) -> Result<Self, Boxed> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let (reached_tx, reached) = watch::channel(None);
        let (discard, discard_rx) = watch::channel(false);
        let control = Arc::new(Control {
            upstream: upstream.trim_end_matches('/').into(),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .build()?,
            armed: AtomicBool::new(false),
            reached: reached_tx,
            discard: discard_rx,
            selected: AtomicUsize::new(0),
            completed: AtomicBool::new(false),
            task: Mutex::new(None),
        });
        let app = axum::Router::new().fallback(forward).with_state(Arc::clone(&control));
        let server = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            url,
            control,
            reached,
            discard,
            server: Some(server),
        })
    }

    pub fn arm(&self) {
        self.control.armed.store(true, Ordering::SeqCst);
    }

    pub fn pending(&self) -> bool {
        self.control.selected.load(Ordering::SeqCst) == 1
            && !self.control.completed.load(Ordering::SeqCst)
            && self.control.task().as_ref().is_some_and(|task| !task.is_finished())
    }

    pub async fn reached(&mut self) -> Result<Binding, Boxed> {
        let binding = tokio::time::timeout(Duration::from_secs(5), self.reached.wait_for(Option::is_some))
            .await
            .context("actual native bootstrap did not reach the held-response gate")??;
        (*binding).context("the held bootstrap reported no native identity")
    }

    pub async fn shutdown(mut self) -> Result<(), Boxed> {
        self.discard.send_replace(true);
        let task = self.control.task().take();
        if let Some(task) = task {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .context("held native bootstrap response did not settle")??;
        }
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
        assert!(
            self.control.completed.load(Ordering::SeqCst),
            "held bootstrap response lost its tracked task"
        );
        Ok(())
    }
}
