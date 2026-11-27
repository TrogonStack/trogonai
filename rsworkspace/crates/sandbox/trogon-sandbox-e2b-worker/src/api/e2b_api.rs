use std::time::Duration;

use reqwest::Method;
use reqwest::header::HeaderValue;
use serde::Serialize;
use serde::de::DeserializeOwned;

use super::build_wire::BuildWire;
use super::connect_request::ConnectRequest;
use super::constants::{
    API_KEY_HEADER, CONNECT_TIMEOUT, INVALID_ID_STATUS, MAX_CREATE_SNAPSHOT_REPLY_BYTES, MAX_ID_BYTES,
    MAX_RESOLVE_SNAPSHOT_REPLY_BYTES, READY_BUILD_STATUS, UNNAMED_SNAPSHOT_SUFFIX,
};
use super::created_snapshot_wire::CreatedSnapshotWire;
use super::memory_request::MemoryRequest;
use super::template_wire::TemplateWire;
use super::{ApiError, CreatedSnapshot, E2bConfig, NativeSandboxAccess, NewSandbox, SandboxDetail, SandboxInfo};
use crate::snapshots::{SnapshotBinding, SnapshotTemplateId};

/// The E2B control plane: sandbox and snapshot lifecycle over REST.
#[derive(Clone)]
pub struct E2bApi {
    config: E2bConfig,
    key: HeaderValue,
    http: reqwest::Client,
    timeout: Duration,
}

impl E2bApi {
    pub fn new(config: E2bConfig, timeout: Duration) -> Result<Self, ApiError> {
        let key = config.api_key_header();
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .build()
            .map_err(ApiError::transport)?;
        Ok(Self {
            config,
            key,
            http,
            timeout,
        })
    }

    pub fn config(&self) -> &E2bConfig {
        &self.config
    }

    pub(crate) fn timeout(&self) -> Duration {
        self.timeout
    }

    pub(crate) async fn snapshot_is_absent(&self, template_id: &SnapshotTemplateId) -> Result<bool, ApiError> {
        match self
            .send::<()>(Method::GET, &format!("/templates/{}", template_id.as_str()), None)
            .await
        {
            Err(ApiError::NotFound(_)) => Ok(true),
            Ok(_) => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn send<B: Serialize>(&self, method: Method, path: &str, body: Option<&B>) -> Result<bytes::Bytes, ApiError> {
        let url = format!("{}{path}", self.config.api_url.as_str().trim_end_matches('/'));
        let mut request = self
            .http
            .request(method, url)
            .header(API_KEY_HEADER, self.key.clone())
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(ApiError::transport)?;
        let status = response.status();
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse().ok());
        let bytes = response.bytes().await.map_err(ApiError::transport)?;
        if status.is_success() {
            Ok(bytes)
        } else {
            Err(ApiError::from_status(status, retry_after, &bytes))
        }
    }

    fn id_path(id: &str, suffix: &str) -> Result<String, ApiError> {
        let valid = !id.is_empty()
            && id.len() <= MAX_ID_BYTES
            && id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_');
        if !valid {
            return Err(ApiError::Rejected {
                status: INVALID_ID_STATUS,
                message: "invalid sandbox id".to_owned(),
            });
        }
        Ok(format!("/sandboxes/{id}{suffix}"))
    }

    fn parse<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, ApiError> {
        serde_json::from_slice(bytes).map_err(|error| ApiError::Transport(format!("unexpected E2B response: {error}")))
    }

    pub async fn create_sandbox(&self, new: &NewSandbox) -> Result<SandboxInfo, ApiError> {
        let bytes = self.send(Method::POST, "/v2/sandboxes", Some(new)).await?;
        Self::parse(&bytes)
    }

    pub(crate) async fn create_snapshot(&self, id: &str) -> Result<CreatedSnapshot, ApiError> {
        let bytes = self
            .send(
                Method::POST,
                &Self::id_path(id, "/snapshots")?,
                Some(&MemoryRequest::keep_memory()),
            )
            .await?;
        if bytes.len() > MAX_CREATE_SNAPSHOT_REPLY_BYTES {
            return Err(ApiError::Transport(
                "snapshot create response exceeds the byte limit; safe cleanup is unavailable".to_owned(),
            ));
        }
        let created = Self::parse::<CreatedSnapshotWire>(&bytes).map_err(|error| {
            ApiError::Transport(format!(
                "snapshot create response cannot establish canonical cleanup identity: {error}; safe cleanup is unavailable"
            ))
        })?;
        let raw = created
            .id
            .strip_suffix(UNNAMED_SNAPSHOT_SUFFIX)
            .filter(|_| created.names.is_empty())
            .ok_or_else(|| {
                ApiError::Transport(
                    "unnamed snapshot returned no authoritative canonical raw:default identity; safe cleanup is unavailable"
                        .to_owned(),
                )
            })?;
        let template_id = SnapshotTemplateId::try_from(raw.to_owned()).map_err(|error| {
            ApiError::Transport(format!(
                "snapshot create response cannot establish canonical cleanup identity: {error}; safe cleanup is unavailable"
            ))
        })?;
        Ok(CreatedSnapshot { template_id })
    }

    pub(crate) async fn resolve_snapshot(&self, created: &CreatedSnapshot) -> Result<SnapshotBinding, ApiError> {
        let bytes = self
            .send::<()>(
                Method::GET,
                &format!("/templates/{}", created.template_id.as_str()),
                None,
            )
            .await?;
        if bytes.len() > MAX_RESOLVE_SNAPSHOT_REPLY_BYTES {
            return Err(ApiError::Transport(
                "snapshot resolver response exceeds the byte limit".to_owned(),
            ));
        }
        let template = Self::parse::<TemplateWire>(&bytes)?;
        let unique_ready_build = match template.builds.as_slice() {
            [BuildWire { build_id, status }] => *build_id == template.build_id && status == READY_BUILD_STATUS,
            _ => false,
        };
        if template.template_id != created.template_id || template.build_count != 1 || !unique_ready_build {
            return Err(ApiError::Transport(
                "snapshot resolver did not confirm the newly created template and unique ready build".to_owned(),
            ));
        }
        Ok(SnapshotBinding {
            template_id: template.template_id,
            build_id: template.build_id,
        })
    }

    pub(crate) async fn delete_snapshot(&self, template_id: &SnapshotTemplateId) -> Result<(), ApiError> {
        self.send::<()>(Method::DELETE, &format!("/templates/{}", template_id.as_str()), None)
            .await
            .map(|_| ())
    }

    pub async fn get_sandbox(&self, id: &str) -> Result<SandboxDetail, ApiError> {
        let bytes = self.send::<()>(Method::GET, &Self::id_path(id, "")?, None).await?;
        Self::parse(&bytes)
    }

    pub(crate) async fn get_native_access(&self, id: &str) -> Result<NativeSandboxAccess, ApiError> {
        let bytes = self.send::<()>(Method::GET, &Self::id_path(id, "")?, None).await?;
        Self::parse(&bytes)
    }

    /// `Ok(false)` when the sandbox is already gone.
    pub async fn delete_sandbox(&self, id: &str) -> Result<bool, ApiError> {
        match self.send::<()>(Method::DELETE, &Self::id_path(id, "")?, None).await {
            Ok(_) => Ok(true),
            Err(ApiError::NotFound(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// Pauses the sandbox keeping its memory, so processes survive the pause.
    pub async fn pause_sandbox(&self, id: &str) -> Result<(), ApiError> {
        self.send(
            Method::POST,
            &Self::id_path(id, "/pause")?,
            Some(&MemoryRequest::keep_memory()),
        )
        .await
        .map(|_| ())
    }

    /// Resumes a paused sandbox, or returns the running one. `timeout` is the new lifetime
    /// in seconds counted from now.
    pub async fn connect_sandbox(&self, id: &str, timeout: u32) -> Result<SandboxInfo, ApiError> {
        let path = Self::id_path(id, "/connect")?.replacen("/sandboxes/", "/v2/sandboxes/", 1);
        let bytes = self
            .send(Method::POST, &path, Some(&ConnectRequest { timeout }))
            .await?;
        Self::parse(&bytes)
    }
}
