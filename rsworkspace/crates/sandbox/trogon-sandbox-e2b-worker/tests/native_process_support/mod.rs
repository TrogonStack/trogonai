// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use serde_json::json;
use url::Url;

use crate::native_command_support::{Boxed, Guest};

pub const SANDBOX_ID: &str = "owned-production-native";

pub struct Control {
    guest: Arc<Guest>,
    end_at: chrono::DateTime<chrono::Utc>,
    live: AtomicBool,
    pub creates: AtomicUsize,
    pub deletes: AtomicUsize,
    pub connects: AtomicUsize,
    pub lists: AtomicUsize,
    pub gets: AtomicUsize,
    pub foreign_gets: AtomicUsize,
    pub foreign_deletes: AtomicUsize,
    pub product_removed: AtomicBool,
    pub recovered_access: AtomicBool,
    pub allow_lifecycle: AtomicBool,
    pub pauses: AtomicUsize,
    pub resume_timeouts: std::sync::Mutex<Vec<(u64, u64)>>,
    paused: AtomicBool,
}

impl Control {
    pub fn access_token(&self) -> &'static str {
        if self.recovered_access.load(Ordering::SeqCst) {
            "fixture-recovered-access"
        } else {
            "fixture-native-access"
        }
    }

    fn remaining_seconds(&self) -> u64 {
        let now = chrono::DateTime::<chrono::Utc>::from(std::time::SystemTime::now());
        let millis = self.end_at.signed_duration_since(now).num_milliseconds();
        if millis <= 0 { 0 } else { ((millis + 999) / 1000) as u64 }
    }
}

fn authenticated(headers: &HeaderMap) -> bool {
    headers.get("X-API-Key").is_some_and(|value| value == "fixture-key")
}

async fn create(State(control): State<Arc<Control>>, headers: HeaderMap) -> impl IntoResponse {
    if !authenticated(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"message":"fixture authentication required"})),
        );
    }
    if control.creates.fetch_add(1, Ordering::SeqCst) != 0 {
        return (
            StatusCode::CONFLICT,
            Json(json!({"message":"duplicate create is forbidden"})),
        );
    }
    control.live.store(true, Ordering::SeqCst);
    (
        StatusCode::CREATED,
        Json(json!({
            "sandboxID":SANDBOX_ID, "templateID":"base", "envdAccessToken":"fixture-native-access", "domain":"e2b.test",
        })),
    )
}

async fn inspect(State(control): State<Arc<Control>>, Path(id): Path<String>, headers: HeaderMap) -> impl IntoResponse {
    if !authenticated(&headers) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"message":"fixture authentication required"})),
        );
    }
    if id != SANDBOX_ID {
        control.foreign_gets.fetch_add(1, Ordering::SeqCst);
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"message":"foreign native identity"})),
        );
    }
    control.gets.fetch_add(1, Ordering::SeqCst);
    if !control.live.load(Ordering::SeqCst) {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"message":"exact native resource absent"})),
        );
    }
    (
        StatusCode::OK,
        Json(json!({
            "sandboxID":SANDBOX_ID, "state":if control.paused.load(Ordering::SeqCst) { "paused" } else { "running" }, "envdAccessToken":control.access_token(), "domain":"e2b.test",
        "envdVersion":"0.2.0", "endAt":control.end_at.to_rfc3339(),
        })),
    )
}

async fn delete(State(control): State<Arc<Control>>, Path(id): Path<String>, headers: HeaderMap) -> StatusCode {
    if !authenticated(&headers) {
        return StatusCode::UNAUTHORIZED;
    }
    if id != SANDBOX_ID {
        control.foreign_deletes.fetch_add(1, Ordering::SeqCst);
        return StatusCode::NOT_FOUND;
    }
    control.deletes.fetch_add(1, Ordering::SeqCst);
    if control.product_removed.load(Ordering::SeqCst) {
        return StatusCode::NOT_FOUND;
    }
    let guest = Arc::clone(&control.guest);
    match tokio::task::spawn_blocking(move || guest.cleanup()).await {
        Ok(Ok(())) => {
            control.live.store(false, Ordering::SeqCst);
            control.product_removed.store(true, Ordering::SeqCst);
            StatusCode::NO_CONTENT
        }
        _ => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn connect(
    State(control): State<Arc<Control>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> impl IntoResponse {
    control.connects.fetch_add(1, Ordering::SeqCst);
    if !authenticated(&headers) || !control.allow_lifecycle.load(Ordering::SeqCst) || id != SANDBOX_ID {
        return (
            StatusCode::CONFLICT,
            Json(json!({"message":"recovery must not connect"})),
        );
    }
    let body: serde_json::Value = serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
    let timeout = body["timeout"].as_u64().unwrap_or(300);
    let maximum = control.remaining_seconds().saturating_add(1);
    control
        .resume_timeouts
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push((timeout, maximum));
    if timeout == 0 || timeout > maximum {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"message":"resume exceeds the authoritative native deadline"})),
        );
    }
    control.paused.store(false, Ordering::SeqCst);
    (
        StatusCode::OK,
        Json(
            json!({"sandboxID":SANDBOX_ID,"templateID":"base","envdAccessToken":control.access_token(),"domain":"e2b.test"}),
        ),
    )
}

async fn pause(State(control): State<Arc<Control>>, Path(id): Path<String>, headers: HeaderMap) -> StatusCode {
    control.pauses.fetch_add(1, Ordering::SeqCst);
    if !authenticated(&headers) || !control.allow_lifecycle.load(Ordering::SeqCst) || id != SANDBOX_ID {
        return StatusCode::CONFLICT;
    }
    control.paused.store(true, Ordering::SeqCst);
    StatusCode::NO_CONTENT
}

async fn list(State(control): State<Arc<Control>>) -> StatusCode {
    control.lists.fetch_add(1, Ordering::SeqCst);
    StatusCode::SERVICE_UNAVAILABLE
}

pub struct Api {
    pub url: Url,
    pub control: Arc<Control>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Api {
    pub async fn start(guest: Arc<Guest>) -> Result<Self, Boxed> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
        let control = Arc::new(Control {
            guest,
            end_at: chrono::Utc::now() + chrono::Duration::seconds(90),
            live: AtomicBool::new(false),
            creates: AtomicUsize::new(0),
            deletes: AtomicUsize::new(0),
            connects: AtomicUsize::new(0),
            lists: AtomicUsize::new(0),
            gets: AtomicUsize::new(0),
            foreign_gets: AtomicUsize::new(0),
            foreign_deletes: AtomicUsize::new(0),
            product_removed: AtomicBool::new(false),
            recovered_access: AtomicBool::new(false),
            allow_lifecycle: AtomicBool::new(false),
            pauses: AtomicUsize::new(0),
            resume_timeouts: std::sync::Mutex::new(Vec::new()),
            paused: AtomicBool::new(false),
        });
        let app = axum::Router::new()
            .route("/v2/sandboxes", post(create))
            .route("/sandboxes", get(list))
            .route("/sandboxes/{id}", get(inspect).delete(delete))
            .route("/v2/sandboxes/{id}/connect", post(connect))
            .route("/sandboxes/{id}/pause", post(pause))
            .with_state(Arc::clone(&control));
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            url,
            control,
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

impl Drop for Api {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
