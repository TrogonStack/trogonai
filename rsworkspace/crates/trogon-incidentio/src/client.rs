//! HTTP client for the incident.io REST API v2.
//!
//! Covers the operations most useful for automated incident response:
//! - Declaring incidents
//! - Posting timeline updates
//! - Resolving incidents
//! - Reading current incident state

use reqwest::Client;
use serde::{Deserialize, Serialize};

const INCIDENTIO_API_BASE: &str = "https://api.incident.io/v2";

// ── Response types ────────────────────────────────────────────────────────────

/// A declared incident as returned by the incident.io API.
#[derive(Debug, Clone, Deserialize)]
pub struct Incident {
    pub id: String,
    pub name: String,
    pub status: String,
    pub severity: Option<IncidentSeverity>,
    pub permalink: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct IncidentSeverity {
    pub id: String,
    pub name: String,
}

// ── Request types ─────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct CreateIncidentBody<'a> {
    name: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    severity_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    idempotency_key: Option<&'a str>,
    mode: &'a str,
}

#[derive(Debug, Serialize)]
struct CreateUpdateBody<'a> {
    incident_id: &'a str,
    message: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    new_status: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    new_severity_id: Option<&'a str>,
}

// ── Error ─────────────────────────────────────────────────────────────────────

#[derive(Debug)]
pub struct IncidentioError(pub String);

impl std::fmt::Display for IncidentioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for IncidentioError {}

// ── Client ────────────────────────────────────────────────────────────────────

/// HTTP client for the incident.io v2 REST API.
#[derive(Clone)]
pub struct IncidentioClient {
    http: Client,
    api_token: String,
    base_url: String,
}

impl IncidentioClient {
    /// Create a client pointing at the real incident.io API.
    pub fn new(http: Client, api_token: String) -> Self {
        Self {
            http,
            api_token,
            base_url: INCIDENTIO_API_BASE.to_string(),
        }
    }

    /// Create a client with a custom base URL (for tests).
    pub fn with_base_url(http: Client, api_token: String, base_url: String) -> Self {
        Self { http, api_token, base_url }
    }

    /// Declare a new incident and return it.
    pub async fn create_incident(
        &self,
        name: &str,
        summary: Option<&str>,
        severity_id: Option<&str>,
        idempotency_key: Option<&str>,
    ) -> Result<Incident, IncidentioError> {
        let body = CreateIncidentBody {
            name,
            summary,
            severity_id,
            idempotency_key,
            mode: "real",
        };

        let resp = self
            .http
            .post(format!("{}/incidents", self.base_url))
            .bearer_auth(&self.api_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| IncidentioError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(IncidentioError(format!(
                "incident.io API error {status}: {text}"
            )));
        }

        let json: serde_json::Value =
            resp.json().await.map_err(|e| IncidentioError(e.to_string()))?;
        serde_json::from_value(json["incident"].clone())
            .map_err(|e| IncidentioError(format!("Failed to parse incident: {e}")))
    }

    /// Post a timeline update to an existing incident.
    pub async fn post_update(
        &self,
        incident_id: &str,
        message: &str,
        new_status: Option<&str>,
        new_severity_id: Option<&str>,
    ) -> Result<(), IncidentioError> {
        let body = CreateUpdateBody {
            incident_id,
            message,
            new_status,
            new_severity_id,
        };

        let resp = self
            .http
            .post(format!("{}/incident_updates", self.base_url))
            .bearer_auth(&self.api_token)
            .json(&body)
            .send()
            .await
            .map_err(|e| IncidentioError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(IncidentioError(format!(
                "incident.io update error {status}: {text}"
            )));
        }

        Ok(())
    }

    /// Resolve an incident (posts a `"resolved"` status update).
    pub async fn resolve(&self, incident_id: &str) -> Result<(), IncidentioError> {
        self.post_update(incident_id, "Incident resolved.", Some("resolved"), None)
            .await
    }

    /// Fetch the current state of an incident by ID.
    pub async fn get_incident(&self, incident_id: &str) -> Result<Incident, IncidentioError> {
        let resp = self
            .http
            .get(format!("{}/incidents/{}", self.base_url, incident_id))
            .bearer_auth(&self.api_token)
            .send()
            .await
            .map_err(|e| IncidentioError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(IncidentioError(format!(
                "incident.io GET error {status}: {text}"
            )));
        }

        let json: serde_json::Value =
            resp.json().await.map_err(|e| IncidentioError(e.to_string()))?;
        serde_json::from_value(json["incident"].clone())
            .map_err(|e| IncidentioError(format!("Failed to parse incident: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::MockServer;

    fn make_client(base_url: &str) -> IncidentioClient {
        IncidentioClient::with_base_url(
            reqwest::Client::new(),
            "test-token".to_string(),
            base_url.to_string(),
        )
    }

    fn incident_json(id: &str, name: &str) -> serde_json::Value {
        serde_json::json!({
            "incident": {
                "id": id,
                "name": name,
                "status": "triage",
                "severity": null,
                "permalink": null
            }
        })
    }

    #[tokio::test]
    async fn create_incident_success() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/incidents")
                .body_contains("CPU spike");
            then.status(201)
                .header("content-type", "application/json")
                .json_body(incident_json("inc-01", "CPU spike"));
        });

        let client = make_client(&server.base_url());
        let inc = client
            .create_incident("CPU spike", None, None, None)
            .await
            .unwrap();
        assert_eq!(inc.id, "inc-01");
        assert_eq!(inc.status, "triage");
    }

    #[tokio::test]
    async fn create_incident_api_error_returns_err() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST).path("/incidents");
            then.status(422).body("unprocessable");
        });

        let client = make_client(&server.base_url());
        let result = client.create_incident("Bad", None, None, None).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().0.contains("422"));
    }

    #[tokio::test]
    async fn bearer_token_is_sent() {
        let server = MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/incidents")
                .header("authorization", "Bearer test-token");
            then.status(201)
                .header("content-type", "application/json")
                .json_body(incident_json("inc-01", "test"));
        });

        let client = make_client(&server.base_url());
        client
            .create_incident("test", None, None, None)
            .await
            .unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn post_update_success() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/incident_updates")
                .body_contains("all clear");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({"incident_update": {}}));
        });

        let client = make_client(&server.base_url());
        client
            .post_update("inc-01", "all clear", None, None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn post_update_api_error_returns_err() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST).path("/incident_updates");
            then.status(500).body("internal error");
        });

        let client = make_client(&server.base_url());
        let result = client.post_update("inc-01", "msg", None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn resolve_posts_resolved_status() {
        let server = MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/incident_updates")
                .body_contains("resolved");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({"incident_update": {}}));
        });

        let client = make_client(&server.base_url());
        client.resolve("inc-01").await.unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn get_incident_success() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/incidents/inc-01");
            then.status(200)
                .header("content-type", "application/json")
                .json_body(serde_json::json!({
                    "incident": {
                        "id": "inc-01",
                        "name": "Disk full",
                        "status": "investigating",
                        "severity": {"id": "sev-1", "name": "P1"},
                        "permalink": null
                    }
                }));
        });

        let client = make_client(&server.base_url());
        let inc = client.get_incident("inc-01").await.unwrap();
        assert_eq!(inc.name, "Disk full");
        assert_eq!(inc.severity.unwrap().name, "P1");
    }

    #[tokio::test]
    async fn get_incident_not_found_returns_err() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/incidents/inc-99");
            then.status(404).body("not found");
        });

        let client = make_client(&server.base_url());
        let result = client.get_incident("inc-99").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn network_error_returns_err() {
        let client = make_client("http://127.0.0.1:1");
        let result = client.create_incident("test", None, None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn create_incident_with_idempotency_key_sends_key() {
        let server = MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/incidents")
                .body_contains("\"idempotency_key\"")
                .body_contains("my-idem-key-123");
            then.status(201)
                .header("content-type", "application/json")
                .json_body(incident_json("inc-03", "Idempotent incident"));
        });

        let client = make_client(&server.base_url());
        client
            .create_incident("Idempotent incident", None, None, Some("my-idem-key-123"))
            .await
            .unwrap();
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn create_incident_malformed_json_response_returns_err() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::POST).path("/incidents");
            then.status(201)
                .header("content-type", "application/json")
                .body("{not valid json");
        });

        let client = make_client(&server.base_url());
        let result = client.create_incident("test", None, None, None).await;
        assert!(result.is_err(), "malformed JSON response must return Err");
    }

    #[tokio::test]
    async fn create_incident_none_optional_fields_not_in_body() {
        // When summary, severity_id, and idempotency_key are all None, they must
        // NOT appear in the serialised request body (skip_serializing_if = "Option::is_none").
        use axum::{Router, routing::post, body::Bytes as AxumBytes};
        use std::sync::{Arc, Mutex};

        let captured: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
        let cap = captured.clone();
        let app = Router::new().route(
            "/incidents",
            post(move |body: AxumBytes| {
                let cap = cap.clone();
                async move {
                    *cap.lock().unwrap() = Some(body.to_vec());
                    (
                        axum::http::StatusCode::CREATED,
                        axum::Json(serde_json::json!({
                            "incident": {
                                "id": "inc-04", "name": "No optional fields",
                                "status": "triage", "severity": null, "permalink": null
                            }
                        })),
                    )
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.ok(); });

        let client = make_client(&format!("http://127.0.0.1:{port}"));
        client.create_incident("No optional fields", None, None, None).await.unwrap();

        let raw = captured.lock().unwrap().clone().unwrap();
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert!(body.get("summary").is_none(), "summary must be absent when None");
        assert!(body.get("severity_id").is_none(), "severity_id must be absent when None");
        assert!(body.get("idempotency_key").is_none(), "idempotency_key must be absent when None");
    }

    #[tokio::test]
    async fn get_incident_server_error_returns_err() {
        let server = MockServer::start_async().await;
        server.mock(|when, then| {
            when.method(httpmock::Method::GET).path("/incidents/inc-err");
            then.status(500).body("internal server error");
        });

        let client = make_client(&server.base_url());
        let result = client.get_incident("inc-err").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().0.contains("500"));
    }

    #[tokio::test]
    async fn post_update_none_fields_not_in_body() {
        // When new_status and new_severity_id are None, they must NOT appear in
        // the request body (skip_serializing_if = "Option::is_none").
        use axum::{Router, routing::post, body::Bytes as AxumBytes};
        use std::sync::{Arc, Mutex};

        let captured: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
        let cap = captured.clone();
        let app = Router::new().route(
            "/incident_updates",
            post(move |body: AxumBytes| {
                let cap = cap.clone();
                async move {
                    *cap.lock().unwrap() = Some(body.to_vec());
                    axum::Json(serde_json::json!({"incident_update": {}}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.ok(); });

        let client = make_client(&format!("http://127.0.0.1:{port}"));
        client.post_update("inc-05", "Progress update", None, None).await.unwrap();

        let raw = captured.lock().unwrap().clone().unwrap();
        let body: serde_json::Value = serde_json::from_slice(&raw).unwrap();
        assert!(body.get("new_status").is_none(), "new_status must be absent when None");
        assert!(body.get("new_severity_id").is_none(), "new_severity_id must be absent when None");
    }

    #[tokio::test]
    async fn create_incident_with_summary_and_severity() {
        let server = MockServer::start_async().await;
        let mock = server.mock(|when, then| {
            when.method(httpmock::Method::POST)
                .path("/incidents")
                .body_contains("\"summary\"")
                .body_contains("\"severity_id\"");
            then.status(201)
                .header("content-type", "application/json")
                .json_body(incident_json("inc-02", "DB down"));
        });

        let client = make_client(&server.base_url());
        client
            .create_incident("DB down", Some("Database unreachable"), Some("sev-1"), None)
            .await
            .unwrap();
        mock.assert_async().await;
    }
}
