use std::time::Duration;

use base64::Engine;
use reqwest::{Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};
use url::Url;

use crate::constants::{DEFAULT_TIMEOUT, READ_CHUNK_BYTES, READ_CHUNK_SCRIPT, READ_EXIT_NOT_FOUND};
use crate::{
    ExecOutput, FlyApiError, FlyApiToken, FlyAppName, FlyEndpoint, FlySnapshot, FlyVolume, FlyVolumeId, Machine,
    MachineId,
};

/// A client for one Fly app's Machines API.
///
/// It never follows redirects, so the bearer token cannot leave the configured endpoint.
#[derive(Clone)]
pub struct FlyApi {
    endpoint: FlyEndpoint,
    app: FlyAppName,
    token: FlyApiToken,
    http: reqwest::Client,
}

impl FlyApi {
    pub fn new(endpoint: FlyEndpoint, app: FlyAppName, token: FlyApiToken) -> Result<Self, FlyApiError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(FlyApiError::Client)?;
        Ok(Self {
            endpoint,
            app,
            token,
            http,
        })
    }

    pub fn app(&self) -> &FlyAppName {
        &self.app
    }

    fn url(&self, segments: &[&str]) -> Result<Url, FlyApiError> {
        let path = format!("/v1/apps/{}/{}", self.app, segments.join("/"));
        Ok(self.endpoint.url().join(&path)?)
    }

    async fn send(
        &self,
        method: Method,
        url: Url,
        body: Option<&Value>,
        timeout: Duration,
    ) -> Result<Option<Value>, FlyApiError> {
        let mut request = self
            .http
            .request(method.clone(), url)
            .timeout(timeout)
            .header(reqwest::header::AUTHORIZATION, self.token.header())
            .header(reqwest::header::ACCEPT, "application/json");
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await?;
        let status = response.status();
        if status == StatusCode::NOT_FOUND && (method == Method::GET || method == Method::DELETE) {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(FlyApiError::Status(status));
        }
        let bytes = response.bytes().await?;
        if status == StatusCode::NO_CONTENT || bytes.is_empty() {
            return Ok(Some(Value::Null));
        }
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        segments: &[&str],
        body: Option<&Value>,
        timeout: Duration,
        missing: &'static str,
    ) -> Result<T, FlyApiError> {
        let value = self
            .send(method, self.url(segments)?, body, timeout)
            .await?
            .ok_or(FlyApiError::NotFound(missing))?;
        Ok(serde_json::from_value(value)?)
    }

    pub async fn list(&self) -> Result<Vec<Machine>, FlyApiError> {
        self.call(Method::GET, &["machines"], None, DEFAULT_TIMEOUT, "the app")
            .await
    }

    pub async fn create(&self, body: &Value) -> Result<Machine, FlyApiError> {
        self.call(
            Method::POST,
            &["machines"],
            Some(body),
            Duration::from_secs(120),
            "the new Machine",
        )
        .await
    }

    pub async fn get(&self, id: &MachineId) -> Result<Option<Machine>, FlyApiError> {
        let url = self.url(&["machines", id.as_str()])?;
        match self.send(Method::GET, url, None, DEFAULT_TIMEOUT).await? {
            Some(value) => Ok(Some(serde_json::from_value(value)?)),
            None => Ok(None),
        }
    }

    pub async fn start(&self, id: &MachineId) -> Result<(), FlyApiError> {
        self.post_empty(id, "start").await
    }

    /// Snapshots the Machine's memory and pauses it; `start` resumes it.
    pub async fn suspend(&self, id: &MachineId) -> Result<(), FlyApiError> {
        self.post_empty(id, "suspend").await
    }

    async fn post_empty(&self, id: &MachineId, action: &str) -> Result<(), FlyApiError> {
        let url = self.url(&["machines", id.as_str(), action])?;
        self.send(Method::POST, url, Some(&Value::Object(Map::new())), DEFAULT_TIMEOUT)
            .await?;
        Ok(())
    }

    /// Force deletion is documented for running Machines, so no separate stop can race it. A Machine that is
    /// already gone is success.
    pub async fn destroy(&self, id: &MachineId) -> Result<(), FlyApiError> {
        let mut url = self.url(&["machines", id.as_str()])?;
        url.set_query(Some("force=true"));
        self.send(Method::DELETE, url, None, DEFAULT_TIMEOUT).await?;
        Ok(())
    }

    pub async fn exec(
        &self,
        id: &MachineId,
        command: Vec<String>,
        timeout: Duration,
    ) -> Result<ExecOutput, FlyApiError> {
        let mut body = Map::new();
        body.insert("command".into(), Value::from(command));
        body.insert("timeout".into(), Value::from(timeout.as_secs().max(1)));
        let body = Value::Object(body);
        self.call(
            Method::POST,
            &["machines", id.as_str(), "exec"],
            Some(&body),
            timeout + Duration::from_secs(5),
            "the Machine",
        )
        .await
    }

    pub async fn volume(&self, id: &FlyVolumeId) -> Result<FlyVolume, FlyApiError> {
        self.call(
            Method::GET,
            &["volumes", id.as_str()],
            None,
            DEFAULT_TIMEOUT,
            "the volume",
        )
        .await
    }

    pub async fn create_volume(&self, body: &Value) -> Result<FlyVolume, FlyApiError> {
        self.call(
            Method::POST,
            &["volumes"],
            Some(body),
            Duration::from_secs(60),
            "the new volume",
        )
        .await
    }

    pub async fn list_volumes(&self) -> Result<Vec<FlyVolume>, FlyApiError> {
        self.call(Method::GET, &["volumes"], None, DEFAULT_TIMEOUT, "the app")
            .await
    }

    /// Deleting is final and idempotent: a volume that is already gone is success. Fly refuses while a Machine
    /// still has it attached.
    pub async fn delete_volume(&self, id: &FlyVolumeId) -> Result<(), FlyApiError> {
        let url = self.url(&["volumes", id.as_str()])?;
        self.send(Method::DELETE, url, None, DEFAULT_TIMEOUT).await?;
        Ok(())
    }

    /// Starts an on-demand snapshot and returns the id it will be listed under. The snapshot is not usable until
    /// `list_snapshots` shows it.
    pub async fn create_snapshot(&self, volume: &FlyVolumeId) -> Result<String, FlyApiError> {
        let response: Value = self
            .call(
                Method::POST,
                &["volumes", volume.as_str(), "snapshots"],
                None,
                DEFAULT_TIMEOUT,
                "the volume",
            )
            .await?;
        let backup = &response["Msg"]["backup"];
        backup["graph_id"]
            .as_str()
            .or_else(|| backup["id"].as_str())
            .map(str::to_owned)
            .ok_or(FlyApiError::SnapshotWithoutId)
    }

    pub async fn list_snapshots(&self, volume: &FlyVolumeId) -> Result<Vec<FlySnapshot>, FlyApiError> {
        self.call(
            Method::GET,
            &["volumes", volume.as_str(), "snapshots"],
            None,
            DEFAULT_TIMEOUT,
            "the volume",
        )
        .await
    }

    /// Reads the 24 KiB chunk of `path` starting at `offset`, which must sit on a chunk boundary. The flag is
    /// true when the chunk was full, so a further chunk may follow.
    pub async fn read_file_chunk(
        &self,
        id: &MachineId,
        path: &str,
        offset: usize,
    ) -> Result<(Vec<u8>, bool), FlyApiError> {
        if path.is_empty() || path.contains('\0') || !offset.is_multiple_of(READ_CHUNK_BYTES) {
            return Err(FlyApiError::InvalidReadPosition);
        }
        let command = vec![
            "/bin/sh".into(),
            "-c".into(),
            READ_CHUNK_SCRIPT.into(),
            "trogon-sandbox-read".into(),
            path.into(),
            (offset / READ_CHUNK_BYTES).to_string(),
        ];
        let output = self.exec(id, command, Duration::from_secs(15)).await?;
        if output.exit_code == READ_EXIT_NOT_FOUND {
            return Err(FlyApiError::FileNotFound(trogon_sandbox_protocol::FileNotFoundError {
                path: path.into(),
            }));
        }
        if output.exit_code != 0 {
            return Err(FlyApiError::GuestRead(output.exit_code));
        }
        let chunk = base64::engine::general_purpose::STANDARD.decode(&output.stdout)?;
        let full = chunk.len() == READ_CHUNK_BYTES;
        Ok((chunk, full))
    }
}
