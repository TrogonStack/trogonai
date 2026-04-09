//! [`SplitClient`] — HTTP thin client for the Split Evaluator service.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use serde::Deserialize;
use serde_json::Value;
use tracing::{debug, instrument, warn};

use crate::{CONTROL, SplitConfig, error::SplitError};

// ── HTTP abstraction ──────────────────────────────────────────────────────────

/// Raw HTTP response from the transport layer.
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// Response body as text.
    pub body: String,
}

/// Abstraction over the HTTP GET transport used by [`SplitClient`].
///
/// All Split Evaluator calls are GET requests, so a single `get` method is
/// sufficient.  The trait is object-safe enough for monomorphization and is
/// implemented for [`reqwest::Client`] in production and [`MockHttpClient`]
/// in tests.
pub trait HttpClient: Clone + Send + Sync + 'static {
    fn get(
        &self,
        url: &str,
        auth_token: &str,
        query: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>>;
}

impl HttpClient for reqwest::Client {
    fn get(
        &self,
        url: &str,
        auth_token: &str,
        query: Vec<(String, String)>,
    ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
        let ref_params: Vec<(&str, &str)> = query
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        let builder = reqwest::Client::get(self, url)
            .header("Authorization", auth_token.to_string())
            .query(&ref_params);
        Box::pin(async move {
            let resp = builder.send().await.map_err(|e| e.to_string())?;
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            Ok(HttpResponse { status, body })
        })
    }
}

// ── Wire types ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TreatmentResponse {
    treatment: String,
}

#[derive(Deserialize)]
struct TreatmentWithConfigResponse {
    treatment: String,
    config: Option<String>,
}

/// A feature flag treatment with an optional JSON configuration payload.
#[derive(Debug, Clone, PartialEq)]
pub struct TreatmentWithConfig {
    /// The treatment value: `"on"`, `"off"`, `"control"`, or a custom string.
    pub treatment: String,
    /// Optional JSON configuration bundled with the treatment.
    pub config: Option<Value>,
}

// ── SplitClient ───────────────────────────────────────────────────────────────

/// Thin HTTP client that delegates feature-flag evaluation to a running
/// [Split Evaluator] sidecar.
///
/// The type parameter `H` is the HTTP transport.  In production `H =
/// reqwest::Client`; in tests any [`HttpClient`] implementation can be
/// supplied, including [`MockHttpClient`].
///
/// All methods are `async` and return `Result<_, SplitError>`.  On any
/// error or when the evaluator is unavailable, prefer returning
/// [`CONTROL`] to blocking the caller — feature flags should never be
/// on the critical path.
///
/// [Split Evaluator]: https://help.split.io/hc/en-us/articles/360020037072-Split-Evaluator
#[derive(Clone)]
pub struct SplitClient<H = reqwest::Client> {
    base_url: String,
    auth_token: String,
    http: H,
}

// ── Generic constructor (any HttpClient backend) ──────────────────────────────

impl<H: HttpClient> SplitClient<H> {
    /// Create a client with an explicit HTTP transport implementation.
    ///
    /// Use this in tests with [`MockHttpClient`].  In production code prefer
    /// [`SplitClient::new`] which uses `reqwest::Client` automatically.
    pub fn new_with(config: SplitConfig, http: H) -> Self {
        Self {
            base_url: config.evaluator_url.trim_end_matches('/').to_string(),
            auth_token: config.auth_token,
            http,
        }
    }

    // ── Core methods ─────────────────────────────────────────────────────────

    /// Evaluate a single feature flag for a user.
    ///
    /// Returns the treatment string (`"on"`, `"off"`, or a custom variant).
    /// Returns [`CONTROL`] when the flag does not exist or the evaluator is
    /// unreachable.
    ///
    /// # Arguments
    /// - `key`        — unique identifier for the entity being evaluated (user ID, device ID, etc.)
    /// - `flag`       — feature flag name as configured in Split.io
    /// - `attributes` — optional key-value map used for targeting rules
    #[instrument(skip(self, attributes), fields(flag, key))]
    pub async fn get_treatment(
        &self,
        key: &str,
        flag: &str,
        attributes: Option<&HashMap<String, Value>>,
    ) -> Result<String, SplitError> {
        let url = format!("{}/client/get-treatment", self.base_url);

        let mut query = vec![
            ("key".to_string(), key.to_string()),
            ("split-name".to_string(), flag.to_string()),
        ];
        if let Some(attrs) = attributes {
            query.push((
                "attributes".to_string(),
                serde_json::to_string(attrs).unwrap_or_default(),
            ));
        }

        debug!(flag, key, "Evaluating treatment");

        let resp = self
            .http
            .get(&url, &self.auth_token, query)
            .await
            .map_err(SplitError::Http)?;

        if resp.status < 200 || resp.status >= 300 {
            warn!(flag, key, status = resp.status, "Evaluator returned error");
            return Err(SplitError::EvaluatorError {
                status: resp.status,
                body: resp.body,
            });
        }

        let parsed: TreatmentResponse = serde_json::from_str(&resp.body)
            .map_err(|e| SplitError::UnexpectedResponse(e.to_string()))?;
        debug!(flag, key, treatment = %parsed.treatment, "Treatment evaluated");
        Ok(parsed.treatment)
    }

    /// Evaluate multiple feature flags in a single request.
    ///
    /// Returns a map of `flag_name → treatment`.  Missing flags appear with
    /// treatment `"control"`.
    ///
    /// # Arguments
    /// - `key`        — unique identifier for the entity being evaluated
    /// - `flags`      — slice of feature flag names
    /// - `attributes` — optional key-value map used for targeting rules
    #[instrument(skip(self, flags, attributes), fields(key, count = flags.len()))]
    pub async fn get_treatments(
        &self,
        key: &str,
        flags: &[&str],
        attributes: Option<&HashMap<String, Value>>,
    ) -> Result<HashMap<String, String>, SplitError> {
        let url = format!("{}/client/get-treatments", self.base_url);
        let split_names = flags.join(",");

        let mut query = vec![
            ("key".to_string(), key.to_string()),
            ("split-names".to_string(), split_names),
        ];
        if let Some(attrs) = attributes {
            query.push((
                "attributes".to_string(),
                serde_json::to_string(attrs).unwrap_or_default(),
            ));
        }

        debug!(key, flags = ?flags, "Evaluating multiple treatments");

        let resp = self
            .http
            .get(&url, &self.auth_token, query)
            .await
            .map_err(SplitError::Http)?;

        if resp.status < 200 || resp.status >= 300 {
            warn!(
                key,
                status = resp.status,
                "Evaluator returned error for get-treatments"
            );
            return Err(SplitError::EvaluatorError {
                status: resp.status,
                body: resp.body,
            });
        }

        // Response shape: { "FLAG_1": { "treatment": "on" }, "FLAG_2": { "treatment": "off" } }
        let raw: HashMap<String, TreatmentResponse> = serde_json::from_str(&resp.body)
            .map_err(|e| SplitError::UnexpectedResponse(e.to_string()))?;
        Ok(raw.into_iter().map(|(k, v)| (k, v.treatment)).collect())
    }

    /// Evaluate a single feature flag and return its treatment plus any
    /// bundled JSON configuration.
    ///
    /// Use this when the flag carries dynamic config (e.g. a JSON payload
    /// specifying UI colours, rate limits, etc.).
    ///
    /// # Arguments
    /// - `key`        — unique identifier for the entity being evaluated
    /// - `flag`       — feature flag name
    /// - `attributes` — optional targeting attributes
    #[instrument(skip(self, attributes), fields(flag, key))]
    pub async fn get_treatment_with_config(
        &self,
        key: &str,
        flag: &str,
        attributes: Option<&HashMap<String, Value>>,
    ) -> Result<TreatmentWithConfig, SplitError> {
        let url = format!("{}/client/get-treatment-with-config", self.base_url);

        let mut query = vec![
            ("key".to_string(), key.to_string()),
            ("split-name".to_string(), flag.to_string()),
        ];
        if let Some(attrs) = attributes {
            query.push((
                "attributes".to_string(),
                serde_json::to_string(attrs).unwrap_or_default(),
            ));
        }

        debug!(flag, key, "Evaluating treatment with config");

        let resp = self
            .http
            .get(&url, &self.auth_token, query)
            .await
            .map_err(SplitError::Http)?;

        if resp.status < 200 || resp.status >= 300 {
            warn!(
                flag,
                key,
                status = resp.status,
                "Evaluator returned error for get-treatment-with-config"
            );
            return Err(SplitError::EvaluatorError {
                status: resp.status,
                body: resp.body,
            });
        }

        let parsed: TreatmentWithConfigResponse = serde_json::from_str(&resp.body)
            .map_err(|e| SplitError::UnexpectedResponse(e.to_string()))?;

        let config = parsed
            .config
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok());

        Ok(TreatmentWithConfig {
            treatment: parsed.treatment,
            config,
        })
    }

    /// Track a custom event for a user.
    ///
    /// Events are used for metric measurement in Split.io experiments.
    ///
    /// # Arguments
    /// - `key`          — unique identifier for the entity
    /// - `traffic_type` — entity category (e.g. `"user"`, `"account"`) — must be pre-configured in FME
    /// - `event_type`   — event name (e.g. `"purchase_completed"`)
    /// - `value`        — optional numeric measurement (e.g. purchase amount)
    /// - `properties`   — optional key-value metadata
    #[instrument(skip(self, properties), fields(key, traffic_type, event_type))]
    pub async fn track(
        &self,
        key: &str,
        traffic_type: &str,
        event_type: &str,
        value: Option<f64>,
        properties: Option<&HashMap<String, Value>>,
    ) -> Result<(), SplitError> {
        let url = format!("{}/client/track", self.base_url);

        let mut query = vec![
            ("key".to_string(), key.to_string()),
            ("traffic-type".to_string(), traffic_type.to_string()),
            ("event-type".to_string(), event_type.to_string()),
        ];
        if let Some(v) = value {
            query.push(("value".to_string(), v.to_string()));
        }
        if let Some(props) = properties {
            query.push((
                "properties".to_string(),
                serde_json::to_string(props).unwrap_or_default(),
            ));
        }

        debug!(key, traffic_type, event_type, "Tracking event");

        let resp = self
            .http
            .get(&url, &self.auth_token, query)
            .await
            .map_err(SplitError::Http)?;

        if resp.status < 200 || resp.status >= 300 {
            warn!(
                key,
                event_type,
                status = resp.status,
                "Evaluator returned error for track"
            );
            return Err(SplitError::EvaluatorError {
                status: resp.status,
                body: resp.body,
            });
        }

        debug!(key, event_type, "Event tracked");
        Ok(())
    }

    /// Check whether the Split Evaluator is healthy.
    ///
    /// Returns `true` if the evaluator responds with `200 OK`.
    pub async fn is_healthy(&self) -> bool {
        let url = format!("{}/admin/healthcheck", self.base_url);
        self.http
            .get(&url, &self.auth_token, vec![])
            .await
            .map(|r| r.status >= 200 && r.status < 300)
            .unwrap_or(false)
    }

    /// Return the treatment for a flag, or [`CONTROL`] on any error.
    ///
    /// Convenience wrapper that swallows errors — useful when a feature flag
    /// must never block the main flow.
    pub async fn get_treatment_or_control(
        &self,
        key: &str,
        flag: &str,
        attributes: Option<&HashMap<String, Value>>,
    ) -> String {
        match self.get_treatment(key, flag, attributes).await {
            Ok(t) => t,
            Err(e) => {
                warn!(flag, key, error = %e, "Treatment evaluation failed — returning control");
                CONTROL.to_string()
            }
        }
    }
}

// ── reqwest::Client convenience constructors ──────────────────────────────────

impl SplitClient<reqwest::Client> {
    /// Create a new client from [`SplitConfig`] using a fresh `reqwest::Client`.
    pub fn new(config: SplitConfig) -> Self {
        Self::new_with(config, reqwest::Client::new())
    }

    /// Create a new client using an existing [`reqwest::Client`] (useful for
    /// sharing connection pools).
    pub fn with_http_client(config: SplitConfig, http: reqwest::Client) -> Self {
        Self::new_with(config, http)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
pub mod mock {
    //! In-process mock HTTP client for unit tests.
    //!
    //! Use [`MockHttpClient`] to test [`SplitClient`] business logic without
    //! spinning up a real HTTP server.

    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Queued response entry: `(status_code, body_text)`.
    type QueueEntry = Result<(u16, String), String>;

    /// Mock [`HttpClient`] that returns pre-programmed responses in FIFO order.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mock = MockHttpClient::new()
    ///     .enqueue_ok(200, r#"{"treatment":"on"}"#);
    /// let client = SplitClient::new_with(config, mock);
    /// assert_eq!(client.get_treatment("u", "flag", None).await.unwrap(), "on");
    /// ```
    #[derive(Default)]
    pub struct MockHttpClient {
        queue: Arc<Mutex<VecDeque<QueueEntry>>>,
    }

    impl MockHttpClient {
        pub fn new() -> Self {
            Self::default()
        }

        /// Enqueue a successful response.
        pub fn enqueue_ok(self, status: u16, body: impl Into<String>) -> Self {
            self.queue
                .lock()
                .unwrap()
                .push_back(Ok((status, body.into())));
            self
        }

        /// Enqueue a transport-level error (connection refused, timeout, etc.).
        pub fn enqueue_err(self, message: impl Into<String>) -> Self {
            self.queue.lock().unwrap().push_back(Err(message.into()));
            self
        }
    }

    impl Clone for MockHttpClient {
        fn clone(&self) -> Self {
            Self {
                queue: Arc::clone(&self.queue),
            }
        }
    }

    impl HttpClient for MockHttpClient {
        fn get(
            &self,
            _url: &str,
            _auth_token: &str,
            _query: Vec<(String, String)>,
        ) -> Pin<Box<dyn Future<Output = Result<HttpResponse, String>> + Send + '_>> {
            let entry = self
                .queue
                .lock()
                .unwrap()
                .pop_front()
                .expect("MockHttpClient: no response queued");
            Box::pin(async move { entry.map(|(status, body)| HttpResponse { status, body }) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::MockServer;
    use mock::MockHttpClient;

    fn make_config(base_url: &str) -> SplitConfig {
        SplitConfig {
            evaluator_url: base_url.to_string(),
            auth_token: "test-token".to_string(),
        }
    }

    fn make_client(base_url: &str) -> SplitClient {
        SplitClient::new(make_config(base_url))
    }

    // ── MockHttpClient unit tests ─────────────────────────────────────────────

    #[tokio::test]
    async fn mock_get_treatment_happy_path() {
        let mock_http =
            MockHttpClient::new().enqueue_ok(200, r#"{"splitName":"flag","treatment":"on"}"#);
        let client = SplitClient::new_with(make_config("http://unused"), mock_http);
        let t = client.get_treatment("u", "flag", None).await.unwrap();
        assert_eq!(t, "on");
    }

    #[tokio::test]
    async fn mock_get_treatment_evaluator_error_maps_to_evaluator_error() {
        let mock_http = MockHttpClient::new().enqueue_ok(500, "internal error");
        let client = SplitClient::new_with(make_config("http://unused"), mock_http);
        let result = client.get_treatment("u", "flag", None).await;
        assert!(matches!(
            result,
            Err(SplitError::EvaluatorError { status: 500, .. })
        ));
    }

    #[tokio::test]
    async fn mock_get_treatment_transport_error_maps_to_http_error() {
        let mock_http = MockHttpClient::new().enqueue_err("connection refused");
        let client = SplitClient::new_with(make_config("http://unused"), mock_http);
        let result = client.get_treatment("u", "flag", None).await;
        assert!(matches!(result, Err(SplitError::Http(_))));
    }

    #[tokio::test]
    async fn mock_get_treatment_or_control_returns_control_on_transport_error() {
        let mock_http = MockHttpClient::new().enqueue_err("timeout");
        let client = SplitClient::new_with(make_config("http://unused"), mock_http);
        let t = client.get_treatment_or_control("u", "flag", None).await;
        assert_eq!(t, CONTROL);
    }

    #[tokio::test]
    async fn mock_is_healthy_returns_true_on_200() {
        let mock_http = MockHttpClient::new().enqueue_ok(200, "");
        let client = SplitClient::new_with(make_config("http://unused"), mock_http);
        assert!(client.is_healthy().await);
    }

    #[tokio::test]
    async fn mock_is_healthy_returns_false_on_500() {
        let mock_http = MockHttpClient::new().enqueue_ok(500, "");
        let client = SplitClient::new_with(make_config("http://unused"), mock_http);
        assert!(!client.is_healthy().await);
    }

    #[tokio::test]
    async fn mock_is_healthy_returns_false_on_transport_error() {
        let mock_http = MockHttpClient::new().enqueue_err("refused");
        let client = SplitClient::new_with(make_config("http://unused"), mock_http);
        assert!(!client.is_healthy().await);
    }

    #[tokio::test]
    async fn mock_track_returns_ok_on_200() {
        let mock_http = MockHttpClient::new().enqueue_ok(200, "Successfully queued event");
        let client = SplitClient::new_with(make_config("http://unused"), mock_http);
        let result = client.track("u", "user", "purchase", None, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn mock_track_returns_err_on_400() {
        let mock_http = MockHttpClient::new().enqueue_ok(400, "bad request");
        let client = SplitClient::new_with(make_config("http://unused"), mock_http);
        let result = client.track("u", "user", "purchase", None, None).await;
        assert!(matches!(
            result,
            Err(SplitError::EvaluatorError { status: 400, .. })
        ));
    }

    #[tokio::test]
    async fn mock_get_treatments_happy_path() {
        let mock_http = MockHttpClient::new().enqueue_ok(
            200,
            r#"{"flag_a":{"treatment":"on"},"flag_b":{"treatment":"off"}}"#,
        );
        let client = SplitClient::new_with(make_config("http://unused"), mock_http);
        let map = client
            .get_treatments("u", &["flag_a", "flag_b"], None)
            .await
            .unwrap();
        assert_eq!(map.get("flag_a").map(String::as_str), Some("on"));
        assert_eq!(map.get("flag_b").map(String::as_str), Some("off"));
    }

    #[tokio::test]
    async fn mock_get_treatment_with_config_parses_json_config() {
        let mock_http = MockHttpClient::new()
            .enqueue_ok(200, r#"{"treatment":"on","config":"{\"color\":\"blue\"}"}"#);
        let client = SplitClient::new_with(make_config("http://unused"), mock_http);
        let result = client
            .get_treatment_with_config("u", "flag", None)
            .await
            .unwrap();
        assert_eq!(result.treatment, "on");
        assert_eq!(result.config.as_ref().unwrap()["color"], "blue");
    }

    // ── httpmock integration tests (reqwest::Client backend) ─────────────────

    #[tokio::test]
    async fn get_treatment_returns_on() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment")
                .header("authorization", "test-token")
                .query_param("key", "user-123")
                .query_param("split-name", "my_flag");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({ "splitName": "my_flag", "treatment": "on" }));
        });

        let client = make_client(&server.base_url());
        let t = client
            .get_treatment("user-123", "my_flag", None)
            .await
            .unwrap();
        assert_eq!(t, "on");
    }

    #[tokio::test]
    async fn get_treatment_returns_off() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment")
                .query_param("split-name", "my_flag");
            then.status(200)
                .json_body(serde_json::json!({ "treatment": "off" }));
        });

        let client = make_client(&server.base_url());
        let t = client
            .get_treatment("user-456", "my_flag", None)
            .await
            .unwrap();
        assert_eq!(t, "off");
    }

    #[tokio::test]
    async fn get_treatment_returns_custom_variant() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment");
            then.status(200)
                .json_body(serde_json::json!({ "treatment": "blue" }));
        });

        let client = make_client(&server.base_url());
        let t = client.get_treatment("u", "color_test", None).await.unwrap();
        assert_eq!(t, "blue");
    }

    #[tokio::test]
    async fn get_treatment_sends_auth_header() {
        let server = MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment")
                .header("authorization", "test-token");
            then.status(200)
                .json_body(serde_json::json!({ "treatment": "on" }));
        });

        let client = make_client(&server.base_url());
        client.get_treatment("u", "f", None).await.unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_treatment_sends_attributes_as_query_param() {
        let server = MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment")
                .query_param_exists("attributes");
            then.status(200)
                .json_body(serde_json::json!({ "treatment": "on" }));
        });

        let client = make_client(&server.base_url());
        let mut attrs = HashMap::new();
        attrs.insert("plan".to_string(), Value::String("premium".to_string()));
        client.get_treatment("u", "f", Some(&attrs)).await.unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_treatment_returns_err_on_500() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment");
            then.status(500).body("internal error");
        });

        let client = make_client(&server.base_url());
        let result = client.get_treatment("u", "f", None).await;
        assert!(matches!(
            result,
            Err(SplitError::EvaluatorError { status: 500, .. })
        ));
    }

    #[tokio::test]
    async fn get_treatment_returns_err_on_401() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment");
            then.status(401).body("unauthorized");
        });

        let client = make_client(&server.base_url());
        let result = client.get_treatment("u", "f", None).await;
        assert!(matches!(
            result,
            Err(SplitError::EvaluatorError { status: 401, .. })
        ));
    }

    // ── get_treatment_or_control ──────────────────────────────────────────────

    #[tokio::test]
    async fn get_treatment_or_control_returns_treatment_on_success() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment");
            then.status(200)
                .json_body(serde_json::json!({ "treatment": "on" }));
        });

        let client = make_client(&server.base_url());
        let t = client.get_treatment_or_control("u", "f", None).await;
        assert_eq!(t, "on");
    }

    #[tokio::test]
    async fn get_treatment_or_control_returns_control_on_error() {
        // Point at a port where nothing listens.
        let client = make_client("http://127.0.0.1:1");
        let t = client.get_treatment_or_control("u", "f", None).await;
        assert_eq!(t, CONTROL);
    }

    #[tokio::test]
    async fn get_treatment_or_control_returns_control_on_500() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment");
            then.status(500).body("oops");
        });

        let client = make_client(&server.base_url());
        let t = client.get_treatment_or_control("u", "f", None).await;
        assert_eq!(t, CONTROL);
    }

    // ── get_treatments ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_treatments_returns_map() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatments")
                .query_param("split-names", "flag_a,flag_b");
            then.status(200).json_body(serde_json::json!({
                "flag_a": { "treatment": "on" },
                "flag_b": { "treatment": "off" }
            }));
        });

        let client = make_client(&server.base_url());
        let map = client
            .get_treatments("u", &["flag_a", "flag_b"], None)
            .await
            .unwrap();
        assert_eq!(map.get("flag_a").map(String::as_str), Some("on"));
        assert_eq!(map.get("flag_b").map(String::as_str), Some("off"));
    }

    #[tokio::test]
    async fn get_treatments_returns_err_on_non_200() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatments");
            then.status(503).body("unavailable");
        });

        let client = make_client(&server.base_url());
        let result = client.get_treatments("u", &["f"], None).await;
        assert!(matches!(
            result,
            Err(SplitError::EvaluatorError { status: 503, .. })
        ));
    }

    // ── get_treatment_with_config ─────────────────────────────────────────────

    #[tokio::test]
    async fn get_treatment_with_config_parses_json_config() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment-with-config");
            then.status(200).json_body(serde_json::json!({
                "treatment": "on",
                "config": "{\"color\":\"blue\",\"limit\":42}"
            }));
        });

        let client = make_client(&server.base_url());
        let result = client
            .get_treatment_with_config("u", "f", None)
            .await
            .unwrap();
        assert_eq!(result.treatment, "on");
        let cfg = result.config.unwrap();
        assert_eq!(cfg["color"], "blue");
        assert_eq!(cfg["limit"], 42);
    }

    #[tokio::test]
    async fn get_treatment_with_config_handles_null_config() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment-with-config");
            then.status(200).json_body(serde_json::json!({
                "treatment": "off",
                "config": null
            }));
        });

        let client = make_client(&server.base_url());
        let result = client
            .get_treatment_with_config("u", "f", None)
            .await
            .unwrap();
        assert_eq!(result.treatment, "off");
        assert!(result.config.is_none());
    }

    #[tokio::test]
    async fn get_treatment_with_config_returns_err_on_non_200() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment-with-config");
            then.status(404).body("not found");
        });

        let client = make_client(&server.base_url());
        let result = client.get_treatment_with_config("u", "f", None).await;
        assert!(matches!(
            result,
            Err(SplitError::EvaluatorError { status: 404, .. })
        ));
    }

    // ── track ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn track_sends_required_params() {
        let server = MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/track")
                .query_param("key", "user-99")
                .query_param("traffic-type", "user")
                .query_param("event-type", "purchase_completed");
            then.status(200).body("Successfully queued event");
        });

        let client = make_client(&server.base_url());
        client
            .track("user-99", "user", "purchase_completed", None, None)
            .await
            .unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn track_sends_value_when_provided() {
        let server = MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/track")
                .query_param("value", "99.99");
            then.status(200).body("Successfully queued event");
        });

        let client = make_client(&server.base_url());
        client
            .track("u", "user", "purchase", Some(99.99), None)
            .await
            .unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn track_sends_properties_when_provided() {
        let server = MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/track")
                .query_param_exists("properties");
            then.status(200).body("Successfully queued event");
        });

        let client = make_client(&server.base_url());
        let mut props = HashMap::new();
        props.insert("sku".to_string(), Value::String("ABC-123".to_string()));
        client
            .track("u", "user", "view", None, Some(&props))
            .await
            .unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn track_returns_err_on_non_200() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/client/track");
            then.status(400).body("bad request");
        });

        let client = make_client(&server.base_url());
        let result = client.track("u", "user", "ev", None, None).await;
        assert!(matches!(
            result,
            Err(SplitError::EvaluatorError { status: 400, .. })
        ));
    }

    // ── is_healthy ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn is_healthy_returns_true_on_200() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/admin/healthcheck");
            then.status(200);
        });

        let client = make_client(&server.base_url());
        assert!(client.is_healthy().await);
    }

    #[tokio::test]
    async fn is_healthy_returns_false_on_500() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/admin/healthcheck");
            then.status(500);
        });

        let client = make_client(&server.base_url());
        assert!(!client.is_healthy().await);
    }

    #[tokio::test]
    async fn is_healthy_returns_false_on_network_error() {
        let client = make_client("http://127.0.0.1:1");
        assert!(!client.is_healthy().await);
    }

    // ── get_treatments edge cases ─────────────────────────────────────────────

    #[tokio::test]
    async fn get_treatments_with_empty_flags_returns_empty_map() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.method(httpmock::Method::GET)
                    .path("/client/get-treatments")
                    .query_param("split-names", "");
                then.status(200).json_body(serde_json::json!({}));
            })
            .await;

        let client = make_client(&server.base_url());
        let result = client.get_treatments("u", &[], None).await.unwrap();
        assert!(result.is_empty(), "empty flags slice must yield empty map");
        mock.assert_async().await;
    }

    // ── get_treatment_with_config edge cases ──────────────────────────────────

    #[tokio::test]
    async fn get_treatment_with_config_invalid_json_is_silently_discarded() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment-with-config");
            then.status(200).json_body(serde_json::json!({
                "treatment": "on",
                "config": "not valid json {{{"
            }));
        });

        let client = make_client(&server.base_url());
        let result = client
            .get_treatment_with_config("u", "my_flag", None)
            .await
            .unwrap();
        assert_eq!(result.treatment, "on");
        assert!(
            result.config.is_none(),
            "invalid JSON config must be silently discarded, not surfaced as an error"
        );
    }

    // ── trailing slash handling ───────────────────────────────────────────────

    #[tokio::test]
    async fn trailing_slash_in_url_is_stripped() {
        let server = MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::GET)
                .path("/client/get-treatment");
            then.status(200)
                .json_body(serde_json::json!({ "treatment": "on" }));
        });

        let client = make_client(&format!("{}/", server.base_url()));
        client.get_treatment("u", "f", None).await.unwrap();
        mock.assert_async().await;
    }
}
