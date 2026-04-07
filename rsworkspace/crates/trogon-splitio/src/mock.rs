//! In-process mock evaluator for local development and testing.
//!
//! When you don't have a Split.io SDK key, use [`MockEvaluator`] to define
//! flag treatments in code and point [`SplitClient`] at it.
//!
//! ```rust
//! use trogon_splitio::mock::MockEvaluator;
//! use trogon_splitio::{SplitClient, SplitConfig};
//!
//! # async fn example() {
//! // Start a mock evaluator with predefined treatments.
//! let mock = MockEvaluator::new()
//!     .with_flag("new_checkout_flow", "on")
//!     .with_flag("beta_dashboard", "off");
//!
//! let (addr, _handle) = mock.serve().await;
//!
//! // Point the client at the mock.
//! let client = SplitClient::new(SplitConfig {
//!     evaluator_url: format!("http://{addr}"),
//!     auth_token: "any".to_string(),
//! });
//!
//! let enabled = client.is_enabled("user-1", &"new_checkout_flow", None).await;
//! assert!(enabled);
//! # }
//! ```

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    routing::get,
};
use serde::Deserialize;
use tokio::task::JoinHandle;

/// Shared state for the mock HTTP server.
#[derive(Clone, Default)]
struct MockState {
    flags: HashMap<String, String>,
    /// Optional JSON config payload for a flag (serialised string, as the real evaluator returns).
    configs: HashMap<String, String>,
}

/// In-process HTTP server that mimics the Split Evaluator API.
///
/// Flags are defined at construction time via [`with_flag`].  Any flag not
/// explicitly defined returns `"control"`.  All users get the same treatment
/// regardless of key — useful for local development and tests.
///
/// [`with_flag`]: MockEvaluator::with_flag
#[derive(Clone, Default)]
pub struct MockEvaluator {
    flags: HashMap<String, String>,
    configs: HashMap<String, String>,
    /// Track received events so tests can assert they were recorded.
    pub tracked_events: Arc<std::sync::Mutex<Vec<TrackedEvent>>>,
}

/// A single event captured by the mock `/client/track` endpoint.
#[derive(Debug, Clone)]
pub struct TrackedEvent {
    pub key: String,
    pub traffic_type: String,
    pub event_type: String,
    pub value: Option<f64>,
}

impl MockEvaluator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the treatment for a flag name.
    pub fn with_flag(mut self, name: impl Into<String>, treatment: impl Into<String>) -> Self {
        self.flags.insert(name.into(), treatment.into());
        self
    }

    /// Set the treatment and a JSON config payload for a flag.
    ///
    /// `config` should be a valid JSON object (e.g. `serde_json::json!({"color":"blue"})`).
    pub fn with_flag_and_config(
        mut self,
        name: impl Into<String>,
        treatment: impl Into<String>,
        config: serde_json::Value,
    ) -> Self {
        let name = name.into();
        self.flags.insert(name.clone(), treatment.into());
        self.configs.insert(name, config.to_string());
        self
    }

    /// Retrieve the list of events tracked via `/client/track`.
    pub fn tracked_events(&self) -> Vec<TrackedEvent> {
        self.tracked_events.lock().unwrap().clone()
    }

    /// Start the mock HTTP server on a random port.
    ///
    /// Returns the bound address and a handle to the background task.
    /// Drop the handle to stop the server.
    pub async fn serve(self) -> (SocketAddr, JoinHandle<()>) {
        let state = Arc::new(MockState {
            flags: self.flags,
            configs: self.configs,
        });
        let events = self.tracked_events.clone();

        let app = Router::new()
            .route("/client/get-treatment", get(handle_get_treatment))
            .route("/client/get-treatments", get(handle_get_treatments))
            .route(
                "/client/get-treatment-with-config",
                get(handle_get_treatment_with_config),
            )
            .route(
                "/client/track",
                get({
                    let events = events.clone();
                    move |q| handle_track(q, events)
                }),
            )
            .route("/admin/healthcheck", get(handle_health))
            .with_state(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock evaluator");
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.ok();
        });

        (addr, handle)
    }
}

// ── Handlers ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TreatmentParams {
    #[serde(rename = "split-name")]
    split_name: Option<String>,
    #[serde(rename = "split-names")]
    split_names: Option<String>,
}

#[derive(Deserialize)]
struct TrackParams {
    key: Option<String>,
    #[serde(rename = "traffic-type")]
    traffic_type: Option<String>,
    #[serde(rename = "event-type")]
    event_type: Option<String>,
    value: Option<f64>,
}

async fn handle_get_treatment(
    State(state): State<Arc<MockState>>,
    Query(params): Query<TreatmentParams>,
) -> axum::Json<serde_json::Value> {
    let name = params.split_name.as_deref().unwrap_or("");
    let treatment = state
        .flags
        .get(name)
        .cloned()
        .unwrap_or_else(|| "control".to_string());
    axum::Json(serde_json::json!({ "splitName": name, "treatment": treatment }))
}

async fn handle_get_treatments(
    State(state): State<Arc<MockState>>,
    Query(params): Query<TreatmentParams>,
) -> axum::Json<serde_json::Value> {
    let names = params.split_names.as_deref().unwrap_or("");
    let result: serde_json::Map<String, serde_json::Value> = names
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|name| {
            let treatment = state
                .flags
                .get(name)
                .cloned()
                .unwrap_or_else(|| "control".to_string());
            (
                name.to_string(),
                serde_json::json!({ "treatment": treatment }),
            )
        })
        .collect();
    axum::Json(serde_json::Value::Object(result))
}

async fn handle_get_treatment_with_config(
    State(state): State<Arc<MockState>>,
    Query(params): Query<TreatmentParams>,
) -> axum::Json<serde_json::Value> {
    let name = params.split_name.as_deref().unwrap_or("");
    let treatment = state
        .flags
        .get(name)
        .cloned()
        .unwrap_or_else(|| "control".to_string());
    let config = state.configs.get(name).cloned();
    axum::Json(serde_json::json!({
        "splitName": name,
        "treatment": treatment,
        "config": config
    }))
}

async fn handle_track(
    Query(params): Query<TrackParams>,
    events: Arc<std::sync::Mutex<Vec<TrackedEvent>>>,
) -> StatusCode {
    if let (Some(key), Some(tt), Some(et)) = (params.key, params.traffic_type, params.event_type) {
        events.lock().unwrap().push(TrackedEvent {
            key,
            traffic_type: tt,
            event_type: et,
            value: params.value,
        });
    }
    StatusCode::OK
}

async fn handle_health() -> StatusCode {
    StatusCode::OK
}

// ── FeatureFlag impl for &str (convenience) ───────────────────────────────────

impl crate::flags::FeatureFlag for &str {
    fn name(&self) -> &'static str {
        // SAFETY: only valid for 'static str literals used in tests/dev.
        // For production code, always define a proper enum.
        Box::leak(self.to_string().into_boxed_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SplitClient, SplitConfig};

    fn client_for(addr: SocketAddr) -> SplitClient {
        SplitClient::new(SplitConfig {
            evaluator_url: format!("http://{addr}"),
            auth_token: "any".to_string(),
        })
    }

    #[tokio::test]
    async fn mock_returns_defined_treatment() {
        let mock = MockEvaluator::new().with_flag("my_flag", "on");
        let (addr, _h) = mock.serve().await;
        let client = client_for(addr);

        let t = client
            .get_treatment("user-1", "my_flag", None)
            .await
            .unwrap();
        assert_eq!(t, "on");
    }

    #[tokio::test]
    async fn mock_returns_control_for_undefined_flag() {
        let mock = MockEvaluator::new();
        let (addr, _h) = mock.serve().await;
        let client = client_for(addr);

        let t = client
            .get_treatment("user-1", "unknown_flag", None)
            .await
            .unwrap();
        assert_eq!(t, "control");
    }

    #[tokio::test]
    async fn mock_returns_off_treatment() {
        let mock = MockEvaluator::new().with_flag("beta", "off");
        let (addr, _h) = mock.serve().await;
        let client = client_for(addr);

        assert!(!client.is_enabled("user-1", &"beta", None).await);
    }

    #[tokio::test]
    async fn mock_is_healthy() {
        let mock = MockEvaluator::new();
        let (addr, _h) = mock.serve().await;
        let client = client_for(addr);

        assert!(client.is_healthy().await);
    }

    #[tokio::test]
    async fn mock_get_treatments_returns_map() {
        let mock = MockEvaluator::new()
            .with_flag("flag_a", "on")
            .with_flag("flag_b", "off");
        let (addr, _h) = mock.serve().await;
        let client = client_for(addr);

        let map = client
            .get_treatments("u", &["flag_a", "flag_b", "flag_c"], None)
            .await
            .unwrap();
        assert_eq!(map["flag_a"], "on");
        assert_eq!(map["flag_b"], "off");
        assert_eq!(map["flag_c"], "control");
    }

    #[tokio::test]
    async fn mock_multiple_users_get_same_treatment() {
        let mock = MockEvaluator::new().with_flag("rollout", "on");
        let (addr, _h) = mock.serve().await;
        let client = client_for(addr);

        // Mock doesn't do per-user targeting — all users get the same treatment
        for user in ["user-1", "user-2", "user-999"] {
            let t = client.get_treatment(user, "rollout", None).await.unwrap();
            assert_eq!(t, "on", "user {user} should get 'on'");
        }
    }

    #[tokio::test]
    async fn mock_get_treatment_with_config_returns_treatment_and_config() {
        let mock = MockEvaluator::new().with_flag_and_config(
            "theme_flag",
            "on",
            serde_json::json!({"color": "blue", "size": 42}),
        );
        let (addr, _h) = mock.serve().await;
        let client = client_for(addr);

        let result = client
            .get_treatment_with_config("user-1", "theme_flag", None)
            .await
            .unwrap();
        assert_eq!(result.treatment, "on");
        let cfg = result.config.unwrap();
        assert_eq!(cfg["color"], "blue");
        assert_eq!(cfg["size"], 42);
    }

    #[tokio::test]
    async fn mock_get_treatment_with_config_returns_null_config_for_plain_flag() {
        let mock = MockEvaluator::new().with_flag("plain_flag", "on");
        let (addr, _h) = mock.serve().await;
        let client = client_for(addr);

        let result = client
            .get_treatment_with_config("user-1", "plain_flag", None)
            .await
            .unwrap();
        assert_eq!(result.treatment, "on");
        assert!(result.config.is_none());
    }

    #[tokio::test]
    async fn mock_track_records_event() {
        let mock = MockEvaluator::new();
        let events = mock.tracked_events.clone();
        let (addr, _h) = mock.serve().await;
        let client = client_for(addr);

        client
            .track("user-1", "user", "purchase_completed", Some(49.99), None)
            .await
            .unwrap();

        let evs = events.lock().unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].key, "user-1");
        assert_eq!(evs[0].traffic_type, "user");
        assert_eq!(evs[0].event_type, "purchase_completed");
        assert_eq!(evs[0].value, Some(49.99));
    }

    #[tokio::test]
    async fn mock_track_records_multiple_events() {
        let mock = MockEvaluator::new();
        let events = mock.tracked_events.clone();
        let (addr, _h) = mock.serve().await;
        let client = client_for(addr);

        client
            .track("user-1", "user", "page_view", None, None)
            .await
            .unwrap();
        client
            .track("user-2", "user", "signup", None, None)
            .await
            .unwrap();

        let evs = events.lock().unwrap();
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].event_type, "page_view");
        assert_eq!(evs[1].event_type, "signup");
    }

    // ── edge cases ────────────────────────────────────────────────────────────

    /// When `split-name` is absent from the query, the handler falls back to
    /// `""`.  Since `""` is not a registered flag, the response is `"control"`.
    #[tokio::test]
    async fn mock_missing_split_name_returns_control() {
        let mock = MockEvaluator::new();
        let (addr, _h) = mock.serve().await;
        let client = client_for(addr);

        // Pass an empty flag name — equivalent to a missing split-name param.
        let t = client.get_treatment("user-1", "", None).await.unwrap();
        assert_eq!(
            t, "control",
            "empty / missing split-name must return 'control'"
        );
    }

    /// When required track params (`key`, `traffic-type`, `event-type`) are
    /// absent the handler returns 200 but does NOT record the event.
    /// This documents the silent-ignore contract so callers can rely on it.
    #[tokio::test]
    async fn mock_track_with_missing_params_is_silently_ignored() {
        let mock = MockEvaluator::new();
        let events = mock.tracked_events.clone();
        let (addr, _h) = mock.serve().await;

        // Hit the /client/track endpoint with only a partial set of params
        // (no event-type) — the handler must swallow it silently.
        let url = format!("http://{addr}/client/track?key=user-1&traffic-type=user");
        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 200, "track endpoint must always return 200");

        let evs = events.lock().unwrap();
        assert_eq!(
            evs.len(),
            0,
            "incomplete track params must not produce a recorded event"
        );
    }

    /// `get_treatments` with no flags in the `split-names` param returns an
    /// empty map — the mock filters out empty tokens from the comma split.
    #[tokio::test]
    async fn mock_get_treatments_with_empty_names_returns_empty_map() {
        let mock = MockEvaluator::new().with_flag("flag_a", "on");
        let (addr, _h) = mock.serve().await;
        let client = client_for(addr);

        let map = client.get_treatments("u", &[], None).await.unwrap();
        assert!(map.is_empty(), "empty split-names must yield an empty map");
    }
}
