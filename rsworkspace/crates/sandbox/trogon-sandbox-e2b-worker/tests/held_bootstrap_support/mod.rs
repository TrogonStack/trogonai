// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

//! A forwarding proxy that holds back the reply of the first native bootstrap until the test releases it.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use serde_json::Value;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::native_command_support::command_deadline_support::TestResult;

#[derive(Clone)]
pub struct Forwarder {
    upstream: String,
    client: reqwest::Client,
    selected: Arc<AtomicBool>,
    /// Notified when the first bootstrap reply is being held.
    pub reached: Arc<Notify>,
    /// Cancelled to let the held reply through.
    pub release: CancellationToken,
}

fn is_bootstrap(path: &str, body: &[u8]) -> bool {
    path == "/process.Process/Start"
        && body
            .get(5..)
            .and_then(|frame| serde_json::from_slice::<Value>(frame).ok())
            .and_then(|value| value["process"]["args"].as_array().cloned())
            .is_some_and(|args| {
                args.iter().any(|arg| {
                    arg.as_str()
                        .is_some_and(|script| script.contains("runner-ready") && script.contains("nohup setsid"))
                })
            })
}

async fn forward(State(state): State<Forwarder>, request: Request) -> Result<Response, StatusCode> {
    let (mut parts, body) = request.into_parts();
    let body = to_bytes(body, 128 * 1024).await.map_err(|_| StatusCode::BAD_REQUEST)?;
    let bootstrap = is_bootstrap(parts.uri.path(), &body);
    parts.headers.remove(header::HOST);
    let response = state
        .client
        .request(parts.method, format!("{}{}", state.upstream, parts.uri))
        .headers(parts.headers)
        .body(body)
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    let status = response.status();
    let mut headers = response.headers().clone();
    headers.remove(header::TRANSFER_ENCODING);
    let body = response.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    if bootstrap && !state.selected.swap(true, Ordering::SeqCst) {
        state.reached.notify_one();
        state.release.cancelled().await;
    }
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(response)
}

pub struct HeldBootstrap {
    pub url: String,
    pub state: Forwarder,
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl HeldBootstrap {
    pub async fn start(upstream: String) -> TestResult<Self> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let state = Forwarder {
            upstream,
            client: reqwest::Client::builder().no_proxy().build()?,
            selected: Arc::new(AtomicBool::new(false)),
            reached: Arc::new(Notify::new()),
            release: CancellationToken::new(),
        };
        let app = Router::new().fallback(forward).with_state(state.clone());
        let stop = CancellationToken::new();
        let stopped = stop.clone().cancelled_owned();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).with_graceful_shutdown(stopped).await;
        });
        Ok(Self { url, state, stop, task })
    }

    pub async fn shutdown(self) {
        self.state.release.cancel();
        self.stop.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(3), self.task).await;
    }
}
