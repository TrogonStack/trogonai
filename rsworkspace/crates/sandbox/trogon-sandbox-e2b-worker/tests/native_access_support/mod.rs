// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use url::Url;

use crate::native_process_support::Control;

#[derive(Clone)]
struct Forwarder {
    upstream: String,
    client: reqwest::Client,
    control: Arc<Control>,
    recovered: Arc<AtomicUsize>,
    rejected: Arc<AtomicUsize>,
}

async fn forward(State(state): State<Forwarder>, request: Request) -> Result<Response, StatusCode> {
    let authorized = request
        .headers()
        .get("X-Access-Token")
        .is_some_and(|value| value == state.control.access_token());
    // A wrong token is counted rather than refused, so the tests can prove none was ever presented.
    state.rejected.fetch_add(usize::from(!authorized), Ordering::SeqCst);
    if state.control.recovered_access.load(Ordering::SeqCst) {
        state.recovered.fetch_add(1, Ordering::SeqCst);
    }
    let (mut parts, body) = request.into_parts();
    let body = to_bytes(body, 128 * 1024).await.map_err(|_| StatusCode::BAD_REQUEST)?;
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
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(response)
}

pub struct NativeAccess {
    pub url: Url,
    pub recovered: Arc<AtomicUsize>,
    pub rejected: Arc<AtomicUsize>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl NativeAccess {
    pub async fn start(
        upstream: String,
        control: Arc<Control>,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
        let recovered = Arc::new(AtomicUsize::new(0));
        let rejected = Arc::new(AtomicUsize::new(0));
        let state = Forwarder {
            upstream,
            client: reqwest::Client::builder().no_proxy().build()?,
            control,
            recovered: Arc::clone(&recovered),
            rejected: Arc::clone(&rejected),
        };
        let app = axum::Router::new().fallback(forward).with_state(state);
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            url,
            recovered,
            rejected,
            task: Some(task),
        })
    }

    pub async fn shutdown(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for NativeAccess {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
