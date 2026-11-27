use std::time::Duration;

use reqwest::header::{ACCEPT, AUTHORIZATION};
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use url::Url;

use crate::constants::{CONTROL_TIMEOUT, KILL_TIMEOUT, MAX_ATTEMPTS, MAX_BODY_BYTES, READ_TIMEOUT};
use crate::{
    Accepted, ApiError, Attach, CommandCreated, EventsUrlError, LifecycleAction, Operation, SandboxInfo, SpaceApiError,
    SpaceId, SpaceOrigin, SpaceToken,
};

/// A client for SPACE's control plane and per-command data plane.
///
/// Neither connection follows redirects or an inherited proxy, and the management
/// token and a command's attach token each go only to their own origin.
#[derive(Clone)]
pub struct SpaceApi {
    control: SpaceOrigin,
    data: SpaceOrigin,
    authorization: SpaceToken,
    http: reqwest::Client,
    stream: reqwest::Client,
    retry_base: Duration,
}

impl SpaceApi {
    /// Builds a client for the two origins, authenticated with the management token.
    pub fn new(control: SpaceOrigin, data: SpaceOrigin, token: SpaceToken) -> Result<Self, SpaceApiError> {
        let builder = || {
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .connect_timeout(Duration::from_secs(10))
        };
        Ok(Self {
            control,
            data,
            authorization: token,
            http: builder().timeout(CONTROL_TIMEOUT).build()?,
            stream: builder().build()?,
            retry_base: Duration::from_millis(200),
        })
    }

    /// Replaces the first delay of the bounded same-key retry, which doubles per attempt.
    pub fn with_retry_base(mut self, retry_base: Duration) -> Self {
        self.retry_base = retry_base;
        self
    }

    /// The first delay of the bounded same-key retry.
    pub fn retry_base(&self) -> Duration {
        self.retry_base
    }

    async fn send_once(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
        key: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, ApiError> {
        let mut url = self.control.url().clone();
        url.set_path(path);
        if !query.is_empty() {
            url.query_pairs_mut().extend_pairs(query);
        }
        let mut request = self
            .http
            .request(method, url)
            .timeout(timeout)
            .header(AUTHORIZATION, self.authorization.authorization())
            .header(ACCEPT, "application/json");
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(transport)?;
        let status = response.status();
        let bytes = read_capped(response).await?;
        if status.is_success() {
            if bytes.is_empty() {
                return Ok(Value::Null);
            }
            return serde_json::from_slice(&bytes)
                .map_err(|_| ApiError::Ambiguous("success reply was not JSON".into()));
        }
        Err(envelope(status, &bytes))
    }

    /// Sends a call and, while the failure leaves the same request safe to repeat,
    /// sends it again with the same key. Only calls SPACE deduplicates, or that change
    /// nothing, go through here.
    async fn send_retrying(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, &str)],
        body: Option<&Value>,
        key: Option<&str>,
        timeout: Duration,
    ) -> Result<Value, ApiError> {
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.send_once(method.clone(), path, query, body, key, timeout).await {
                Err(error) if error.is_retryable() && attempt < MAX_ATTEMPTS => {
                    tokio::time::sleep(self.retry_base * 2u32.saturating_pow(attempt - 1)).await;
                }
                other => return other,
            }
        }
    }

    fn encode(body: &impl Serialize) -> Result<Value, ApiError> {
        serde_json::to_value(body).map_err(|e| ApiError::Protocol(e.to_string()))
    }

    fn typed<T: DeserializeOwned>(value: Value) -> Result<T, ApiError> {
        serde_json::from_value(value).map_err(|e| ApiError::Protocol(e.to_string()))
    }

    /// `POST /v1/sandboxes`. SPACE only accepts the create; poll the returned operation
    /// for the outcome.
    pub async fn create_sandbox(&self, body: &impl Serialize, key: &str) -> Result<Accepted, ApiError> {
        let body = Self::encode(body)?;
        let reply = self
            .send_retrying(
                Method::POST,
                "/v1/sandboxes",
                &[],
                Some(&body),
                Some(key),
                CONTROL_TIMEOUT,
            )
            .await?;
        Self::typed(reply)
    }

    /// `GET /v1/operations/{id}`.
    pub async fn operation(&self, id: &SpaceId) -> Result<Operation, ApiError> {
        let reply = self
            .send_retrying(
                Method::GET,
                &format!("/v1/operations/{id}"),
                &[],
                None,
                None,
                READ_TIMEOUT,
            )
            .await?;
        Self::typed(reply)
    }

    /// `GET /v1/sandboxes/{id}`; `None` once SPACE no longer knows it.
    pub async fn sandbox(&self, id: &SpaceId) -> Result<Option<SandboxInfo>, ApiError> {
        match self
            .send_retrying(
                Method::GET,
                &format!("/v1/sandboxes/{id}"),
                &[],
                None,
                None,
                READ_TIMEOUT,
            )
            .await
        {
            Ok(reply) => Self::typed(reply).map(Some),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// One page of `GET /v1/sandboxes`, and the cursor of the next page.
    pub async fn list_sandboxes(&self, cursor: Option<&str>) -> Result<(Vec<SandboxInfo>, Option<String>), ApiError> {
        let mut query = vec![("limit", "100")];
        if let Some(cursor) = cursor {
            query.push(("cursor", cursor));
        }
        let page = self
            .send_retrying(Method::GET, "/v1/sandboxes", &query, None, None, READ_TIMEOUT)
            .await?;
        let next = page["next_cursor"].as_str().map(str::to_owned);
        let data = Self::typed(page["data"].clone())?;
        Ok((data, next))
    }

    /// `GET /v1/sandboxes/{id}/processes?limit=1`, which only answers once the guest
    /// agent is reachable.
    pub async fn probe_processes(&self, id: &SpaceId, timeout: Duration) -> Result<(), ApiError> {
        let reply = self
            .send_retrying(
                Method::GET,
                &format!("/v1/sandboxes/{id}/processes"),
                &[("limit", "1")],
                None,
                None,
                timeout,
            )
            .await?;
        if reply["data"].is_array() {
            Ok(())
        } else {
            Err(ApiError::Protocol("process readiness reply had no list".into()))
        }
    }

    /// `POST /v1/sandboxes/{id}:pause|:suspend|:wake`.
    pub async fn lifecycle(&self, id: &SpaceId, action: LifecycleAction, key: &str) -> Result<Accepted, ApiError> {
        let reply = self
            .send_retrying(
                Method::POST,
                &format!("/v1/sandboxes/{id}:{}", action.verb()),
                &[],
                None,
                Some(key),
                CONTROL_TIMEOUT,
            )
            .await?;
        Self::typed(reply)
    }

    /// `DELETE /v1/sandboxes/{id}`; `None` when SPACE no longer has it.
    pub async fn delete_sandbox(&self, id: &SpaceId, key: &str) -> Result<Option<Accepted>, ApiError> {
        match self
            .send_retrying(
                Method::DELETE,
                &format!("/v1/sandboxes/{id}"),
                &[],
                None,
                Some(key),
                CONTROL_TIMEOUT,
            )
            .await
        {
            Ok(reply) => Self::typed(reply).map(Some),
            Err(error) if error.is_not_found() => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// `POST /v1/sandboxes/{id}/commands`. Sent exactly once: a failure here leaves it
    /// unknown whether a process was created, and the caller must not send it again.
    pub async fn create_command(
        &self,
        id: &SpaceId,
        body: &impl Serialize,
        key: &str,
    ) -> Result<CommandCreated, ApiError> {
        let body = Self::encode(body)?;
        let reply = self
            .send_once(
                Method::POST,
                &format!("/v1/sandboxes/{id}/commands"),
                &[],
                Some(&body),
                Some(key),
                CONTROL_TIMEOUT,
            )
            .await?;
        Self::typed(reply)
    }

    /// `POST /v1/sandboxes/{id}/processes/{process}:kill`. It does not deduplicate by
    /// key, so it is sent once.
    pub async fn kill_process(&self, id: &SpaceId, process: &SpaceId) -> Result<(), ApiError> {
        self.send_once(
            Method::POST,
            &format!("/v1/sandboxes/{id}/processes/{process}:kill"),
            &[],
            None,
            None,
            KILL_TIMEOUT,
        )
        .await
        .map(|_| ())
    }

    /// Resolves an `events_url` against the data-plane origin and refuses any other
    /// destination, so the attach token only goes where it is meant to.
    pub fn events_url(&self, events_url: &str) -> Result<Url, EventsUrlError> {
        let url = if events_url.starts_with('/') && !events_url.starts_with("//") {
            self.data.url().join(events_url)?
        } else {
            Url::parse(events_url)?
        };
        if self.data.admits(&url) {
            Ok(url)
        } else {
            Err(EventsUrlError::OffOrigin)
        }
    }

    /// Opens the command's event stream. Attaching starts the process. The reply is
    /// returned unread so the caller can apply its own deadline.
    pub async fn open_events(
        &self,
        attach: &Attach,
        events_url: &str,
        last_event_id: Option<&str>,
    ) -> Result<reqwest::Response, ApiError> {
        let url = self
            .events_url(events_url)
            .map_err(|e| ApiError::Protocol(e.to_string()))?;
        let mut request = self
            .stream
            .get(url)
            .header(AUTHORIZATION, attach.token.authorization())
            .header(ACCEPT, "text/event-stream");
        if let Some(id) = last_event_id {
            request = request.header("Last-Event-ID", id);
        }
        let response = request.send().await.map_err(transport)?;
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let bytes = read_capped(response).await?;
        Err(envelope(status, &bytes))
    }
}

fn transport(error: reqwest::Error) -> ApiError {
    if error.is_connect() {
        ApiError::NotApplied(error.without_url().to_string())
    } else {
        ApiError::Ambiguous(error.without_url().to_string())
    }
}

async fn read_capped(mut response: reqwest::Response) -> Result<Vec<u8>, ApiError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(transport)? {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(ApiError::Ambiguous("reply body is too large".into()));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Reads SPACE's error envelope, which may sit at the top level or under an `error` key.
fn envelope(status: StatusCode, bytes: &[u8]) -> ApiError {
    let parsed: Value = serde_json::from_slice(bytes).unwrap_or(Value::Null);
    let body = if parsed["error"].is_object() {
        &parsed["error"]
    } else {
        &parsed
    };
    let transient = matches!(status.as_u16(), 429 | 502 | 503 | 504);
    ApiError::Status {
        status: status.as_u16(),
        code: body["code"]
            .as_str()
            .map_or_else(|| format!("http_{}", status.as_u16()), str::to_owned),
        message: body["message"]
            .as_str()
            .unwrap_or("no message")
            .chars()
            .take(500)
            .collect(),
        retryable: body["retryable"].as_bool().unwrap_or(transient),
        request_id: body["request_id"].as_str().map(str::to_owned),
        details: Box::new(body["details"].clone()),
    }
}
