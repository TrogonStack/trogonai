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

use axum::{Router, extract::{Query, State}, http::StatusCode, routing::get};
use serde::Deserialize;
use tokio::task::JoinHandle;

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

    /// Start the mock HTTP server on a random port.
    ///
    /// Returns the bound address and a handle to the background task.
    /// Drop the handle to stop the server.
    pub async fn serve(self) -> (SocketAddr, JoinHandle<()>) {
        let state = Arc::new(self.flags);

        let app = Router::new()
            .route("/client/get-treatment", get(handle_get_treatment))
            .route("/client/get-treatments", get(handle_get_treatments))
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

async fn handle_get_treatment(
    State(flags): State<Arc<HashMap<String, String>>>,
    Query(params): Query<TreatmentParams>,
) -> axum::Json<serde_json::Value> {
    let name = params.split_name.as_deref().unwrap_or("");
    let treatment = flags.get(name).cloned().unwrap_or_else(|| "control".to_string());
    axum::Json(serde_json::json!({ "splitName": name, "treatment": treatment }))
}

async fn handle_get_treatments(
    State(flags): State<Arc<HashMap<String, String>>>,
    Query(params): Query<TreatmentParams>,
) -> axum::Json<serde_json::Value> {
    let names = params.split_names.as_deref().unwrap_or("");
    let result: serde_json::Map<String, serde_json::Value> = names
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|name| {
            let treatment = flags.get(name).cloned().unwrap_or_else(|| "control".to_string());
            (name.to_string(), serde_json::json!({ "treatment": treatment }))
        })
        .collect();
    axum::Json(serde_json::Value::Object(result))
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

        let t = client.get_treatment("user-1", "my_flag", None).await.unwrap();
        assert_eq!(t, "on");
    }

    #[tokio::test]
    async fn mock_returns_control_for_undefined_flag() {
        let mock = MockEvaluator::new();
        let (addr, _h) = mock.serve().await;
        let client = client_for(addr);

        let t = client.get_treatment("user-1", "unknown_flag", None).await.unwrap();
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

        let map = client.get_treatments("u", &["flag_a", "flag_b", "flag_c"], None).await.unwrap();
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
}
