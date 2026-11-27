// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

//! A facade in front of the fake native control plane that rewrites the response of the exact sandbox lookup.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::Response;
use serde_json::{Value, json};
use url::Url;

use crate::ensure_support::Boxed;
use crate::native_process_support::SANDBOX_ID;

#[derive(Clone, Copy, Debug)]
pub enum Fault {
    ForeignIdentity,
    NotFound,
    Unauthorized,
    Unavailable,
    Paused,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Counts {
    pub exact_gets: usize,
    pub upstream_successes: usize,
    pub injected: usize,
}

#[derive(Default)]
struct Control {
    fault: Mutex<Option<Fault>>,
    exact_gets: AtomicUsize,
    upstream_successes: AtomicUsize,
    injected: AtomicUsize,
}

#[derive(Clone)]
struct Forwarder {
    upstream: String,
    client: reqwest::Client,
    control: Arc<Control>,
}

fn gateway<E>(_: E) -> StatusCode {
    StatusCode::BAD_GATEWAY
}

/// The status and body the injected fault substitutes for the genuine running-sandbox answer.
fn injected(fault: Fault, mut value: Value) -> (StatusCode, Value) {
    let refusal = json!({"message":"owned recovery response fixture refusal"});
    match fault {
        Fault::ForeignIdentity => {
            value["sandboxID"] = json!("foreign-native-sandbox");
            (StatusCode::OK, value)
        }
        Fault::Paused => {
            value["state"] = json!("paused");
            (StatusCode::OK, value)
        }
        Fault::NotFound => (StatusCode::NOT_FOUND, refusal),
        Fault::Unauthorized => (StatusCode::UNAUTHORIZED, refusal),
        Fault::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, refusal),
    }
}

async fn forward(State(state): State<Forwarder>, request: Request) -> Result<Response, StatusCode> {
    let exact = request.method() == Method::GET && request.uri().path() == format!("/sandboxes/{SANDBOX_ID}");
    let fault = if exact {
        state.control.exact_gets.fetch_add(1, Ordering::SeqCst);
        *state.control.fault.lock().unwrap_or_else(PoisonError::into_inner)
    } else {
        None
    };
    let (mut parts, body) = request.into_parts();
    let body = to_bytes(body, 128 * 1024).await.map_err(gateway)?;
    parts.headers.remove(header::HOST);
    let upstream = state
        .client
        .request(parts.method, format!("{}{}", state.upstream, parts.uri))
        .headers(parts.headers)
        .body(body)
        .send()
        .await
        .map_err(gateway)?;
    let mut status = upstream.status();
    let mut headers = upstream.headers().clone();
    headers.remove(header::TRANSFER_ENCODING);
    headers.remove(header::CONTENT_LENGTH);
    let mut body = upstream.bytes().await.map_err(gateway)?;
    if exact && status == StatusCode::OK {
        state.control.upstream_successes.fetch_add(1, Ordering::SeqCst);
    }
    if let Some(fault) = fault {
        let value: Value = serde_json::from_slice(&body).map_err(gateway)?;
        let (replaced, value) = injected(fault, value);
        status = replaced;
        body = serde_json::to_vec(&value).map_err(gateway)?.into();
        state.control.injected.fetch_add(1, Ordering::SeqCst);
    }
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(response)
}

pub struct Facade {
    pub url: Url,
    control: Arc<Control>,
    task: tokio::task::JoinHandle<()>,
}

impl Facade {
    pub async fn start(upstream: &Url) -> Result<Self, Boxed> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
        let control = Arc::new(Control::default());
        let state = Forwarder {
            upstream: upstream.as_str().trim_end_matches('/').to_owned(),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()?,
            control: Arc::clone(&control),
        };
        let app = axum::Router::new().fallback(forward).with_state(state);
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self { url, control, task })
    }

    pub fn set_fault(&self, fault: Option<Fault>) {
        *self.control.fault.lock().unwrap_or_else(PoisonError::into_inner) = fault;
    }

    pub fn counts(&self) -> Counts {
        Counts {
            exact_gets: self.control.exact_gets.load(Ordering::SeqCst),
            upstream_successes: self.control.upstream_successes.load(Ordering::SeqCst),
            injected: self.control.injected.load(Ordering::SeqCst),
        }
    }

    pub async fn shutdown(self) {
        self.task.abort();
        let _ = self.task.await;
    }
}
