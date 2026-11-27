//! Vercel Sandbox REST client (sandbox create, inspect, stop, resume, delete).
//!
//! There is no vendor SDK here: the calls are plain HTTPS requests authenticated with a bearer token. The token is
//! held only inside a sensitive header value, so it never shows up in `Debug` output or logs.

use std::time::Duration;

use reqwest::{Method, RequestBuilder, Response};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::constants::CONNECT_TIMEOUT;
use crate::json_object::object;
use crate::safe_id::safe_id;
use crate::{NewSandbox, SandboxAndSession, SnapshotRecord, VercelApiError, VercelConfig};

/// The `snapshot` envelope Vercel wraps a created snapshot in.
#[derive(Deserialize)]
struct SnapshotReply {
    snapshot: SnapshotRecord,
}

/// A client for the Vercel Sandbox REST API.
#[derive(Debug, Clone)]
pub struct VercelApi {
    config: VercelConfig,
    http: reqwest::Client,
}

impl VercelApi {
    /// Builds a client whose every request gives up after `timeout`.
    pub fn new(config: VercelConfig, timeout: Duration) -> Result<Self, VercelApiError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .build()
            .map_err(VercelApiError::transport)?;
        Ok(Self { config, http })
    }

    pub fn config(&self) -> &VercelConfig {
        &self.config
    }

    /// A request to `path` carrying the credentials and the team scope.
    pub(crate) fn builder(&self, method: Method, path: &str, query: &[(&str, &str)]) -> RequestBuilder {
        let url = format!("{}{path}", self.config.endpoint().as_str().trim_end_matches('/'));
        let mut scoped: Vec<(&str, &str)> = vec![("teamId", self.config.team_id().as_str())];
        scoped.extend_from_slice(query);
        self.http
            .request(method, url)
            .query(&scoped)
            .header(reqwest::header::AUTHORIZATION, self.config.token().header())
            .header(reqwest::header::ACCEPT, "application/json")
    }

    /// Turns a non-2xx response into the matching [`VercelApiError`].
    pub(crate) async fn check(response: Response) -> Result<Response, VercelApiError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse().ok());
        let bytes = response.bytes().await.map_err(VercelApiError::transport)?;
        Err(VercelApiError::from_status(status, retry_after, &bytes))
    }

    /// Sends a request and returns the body of a successful answer.
    pub(crate) async fn send(&self, request: RequestBuilder) -> Result<bytes::Bytes, VercelApiError> {
        let response = request.send().await.map_err(VercelApiError::transport)?;
        let response = Self::check(response).await?;
        response.bytes().await.map_err(VercelApiError::transport)
    }

    pub(crate) fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, VercelApiError> {
        serde_json::from_slice(bytes).map_err(VercelApiError::unexpected)
    }

    pub async fn create_sandbox(&self, new: &NewSandbox) -> Result<SandboxAndSession, VercelApiError> {
        let endpoint = if new.runtime.is_some() {
            "/v2/sandboxes"
        } else {
            "/v3/sandboxes"
        };
        let bytes = self.send(self.builder(Method::POST, endpoint, &[]).json(new)).await?;
        Self::parse(&bytes)
    }

    /// Fetches a sandbox. With `resume` a stopped persistent sandbox gets a fresh session.
    pub async fn get_sandbox(&self, name: &str, resume: bool) -> Result<SandboxAndSession, VercelApiError> {
        safe_id(name, "sandbox name")?;
        let bytes = self
            .send(self.builder(
                Method::GET,
                &format!("/v2/sandboxes/{name}"),
                &[
                    ("projectId", self.config.project_id().as_str()),
                    ("resume", if resume { "true" } else { "false" }),
                ],
            ))
            .await?;
        Self::parse(&bytes)
    }

    /// Stops a session. A persistent sandbox keeps its files for the next session.
    pub async fn stop_session(&self, session_id: &str) -> Result<(), VercelApiError> {
        safe_id(session_id, "session id")?;
        // The endpoint answers 415 to a POST without a JSON body.
        self.send(
            self.builder(Method::POST, &format!("/v2/sandboxes/sessions/{session_id}/stop"), &[])
                .json(&Value::Object(Map::new())),
        )
        .await
        .map(|_| ())
    }

    /// Snapshots a running session's filesystem. Vercel terminates the session once the snapshot is taken.
    /// `expiration_ms` of `None` takes Vercel's default (7 days); `Some(0)` never expires.
    pub async fn snapshot_session(
        &self,
        session_id: &str,
        expiration_ms: Option<u64>,
    ) -> Result<SnapshotRecord, VercelApiError> {
        safe_id(session_id, "session id")?;
        let body = match expiration_ms {
            Some(ms) => object([("expiration", Value::from(ms))]),
            None => Value::Object(Map::new()),
        };
        let bytes = self
            .send(
                self.builder(
                    Method::POST,
                    &format!("/v3/sandboxes/sessions/{session_id}/snapshot"),
                    &[],
                )
                .json(&body),
            )
            .await?;
        let reply: SnapshotReply = Self::parse(&bytes)?;
        Ok(reply.snapshot)
    }

    /// `Ok(false)` when the sandbox is already gone.
    pub async fn delete_sandbox(&self, name: &str) -> Result<bool, VercelApiError> {
        safe_id(name, "sandbox name")?;
        match self
            .send(self.builder(
                Method::DELETE,
                &format!("/v2/sandboxes/{name}"),
                &[("projectId", self.config.project_id().as_str())],
            ))
            .await
        {
            Ok(_) => Ok(true),
            Err(VercelApiError::NotFound(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }
}
