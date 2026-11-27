//! Daytona REST client: sandbox lifecycle on the control plane, and process and file calls through the sandbox's
//! toolbox proxy.
//!
//! The key is held only inside a sensitive header value, so it never shows up in `Debug` output or logs. Only a 404
//! confirms that something is absent; a 403, a 5xx or a transport failure stays an error.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use reqwest::{Method, RequestBuilder, Response};

use crate::constants::{CONNECT_TIMEOUT, PROXY_CACHE_LIMIT, TOOLBOX_DEADLINE_SLACK};
use crate::multipart::file_part;
use crate::proxy_url::ProxyUrl;
use crate::safe_id::safe_id;
use crate::{
    DaytonaApiError, DaytonaConfig, ExecuteReply, ExecuteRequest, GitCloneRequest, NewSandbox, PortPreview,
    SandboxRecord,
};

/// A client for the Daytona REST API.
#[derive(Debug, Clone)]
pub struct DaytonaApi {
    config: DaytonaConfig,
    http: reqwest::Client,
    timeout: Duration,
    proxies: Arc<Mutex<HashMap<String, String>>>,
}

impl DaytonaApi {
    /// Builds a client whose every request gives up after `timeout`.
    pub fn new(config: DaytonaConfig, timeout: Duration) -> Result<Self, DaytonaApiError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .build()
            .map_err(DaytonaApiError::transport)?;
        Ok(Self {
            config,
            http,
            timeout,
            proxies: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn config(&self) -> &DaytonaConfig {
        &self.config
    }

    /// A request to `url` carrying the credentials and the organization scope.
    fn builder(&self, method: Method, url: String) -> RequestBuilder {
        let request = self
            .http
            .request(method, url)
            .header(reqwest::header::AUTHORIZATION, self.config.api_key().header())
            .header(reqwest::header::ACCEPT, "application/json");
        match self.config.organization() {
            Some(organization) => request.header("X-Daytona-Organization-ID", organization.header()),
            None => request,
        }
    }

    fn control(&self, method: Method, path: &str) -> RequestBuilder {
        let base = self.config.endpoint().as_str().trim_end_matches('/');
        self.builder(method, format!("{base}{path}"))
    }

    /// Turns a non-2xx response into the matching [`DaytonaApiError`].
    async fn check(response: Response) -> Result<Response, DaytonaApiError> {
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let retry_after = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse().ok());
        let bytes = response.bytes().await.map_err(DaytonaApiError::transport)?;
        Err(DaytonaApiError::from_status(status, retry_after, &bytes))
    }

    async fn send(request: RequestBuilder) -> Result<Bytes, DaytonaApiError> {
        let response = request.send().await.map_err(DaytonaApiError::transport)?;
        let response = Self::check(response).await?;
        response.bytes().await.map_err(DaytonaApiError::transport)
    }

    fn parse<T: serde::de::DeserializeOwned>(bytes: &[u8]) -> Result<T, DaytonaApiError> {
        serde_json::from_slice(bytes).map_err(DaytonaApiError::unexpected)
    }

    pub async fn create_sandbox(&self, new: &NewSandbox) -> Result<SandboxRecord, DaytonaApiError> {
        let bytes = Self::send(self.control(Method::POST, "/sandbox").json(new)).await?;
        Self::parse(&bytes)
    }

    pub async fn get_sandbox(&self, id: &str) -> Result<SandboxRecord, DaytonaApiError> {
        safe_id(id, "sandbox id")?;
        let bytes = Self::send(self.control(Method::GET, &format!("/sandbox/{id}"))).await?;
        Self::parse(&bytes)
    }

    pub async fn start_sandbox(&self, id: &str) -> Result<(), DaytonaApiError> {
        safe_id(id, "sandbox id")?;
        Self::send(self.control(Method::POST, &format!("/sandbox/{id}/start")))
            .await
            .map(|_| ())
    }

    pub async fn stop_sandbox(&self, id: &str) -> Result<(), DaytonaApiError> {
        safe_id(id, "sandbox id")?;
        Self::send(self.control(Method::POST, &format!("/sandbox/{id}/stop")))
            .await
            .map(|_| ())
    }

    /// `Ok(false)` when the sandbox is already gone.
    pub async fn delete_sandbox(&self, id: &str) -> Result<bool, DaytonaApiError> {
        safe_id(id, "sandbox id")?;
        let sent = Self::send(
            self.control(Method::DELETE, &format!("/sandbox/{id}"))
                .query(&[("force", "true")]),
        )
        .await;
        self.proxies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id);
        match sent {
            Ok(_) => Ok(true),
            Err(DaytonaApiError::NotFound(_)) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// The public URL and token for `port` of a sandbox.
    pub async fn port_preview(&self, id: &str, port: u16) -> Result<PortPreview, DaytonaApiError> {
        safe_id(id, "sandbox id")?;
        let bytes = Self::send(self.control(Method::GET, &format!("/sandbox/{id}/port-preview/{port}"))).await?;
        Self::parse(&bytes)
    }

    /// The toolbox proxy of a sandbox, remembered per sandbox.
    async fn proxy_for(&self, id: &str) -> Result<String, DaytonaApiError> {
        safe_id(id, "sandbox id")?;
        let known = self
            .proxies
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(id)
            .cloned();
        if let Some(proxy) = known {
            return Ok(proxy);
        }
        let bytes = Self::send(self.control(Method::GET, &format!("/sandbox/{id}/toolbox-proxy-url"))).await?;
        let proxy = Self::parse::<ProxyUrl>(&bytes)?.url.trim_end_matches('/').to_owned();
        let mut proxies = self.proxies.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if proxies.len() >= PROXY_CACHE_LIMIT {
            proxies.clear();
        }
        proxies.insert(id.to_owned(), proxy.clone());
        Ok(proxy)
    }

    async fn toolbox(&self, method: Method, id: &str, rest: &str) -> Result<RequestBuilder, DaytonaApiError> {
        let proxy = self.proxy_for(id).await?;
        Ok(self.builder(method, format!("{proxy}/{id}/{rest}")))
    }

    /// Runs a command in the sandbox and waits for it. The call may outlast the client timeout by the command's own.
    pub async fn execute(&self, id: &str, request: &ExecuteRequest) -> Result<ExecuteReply, DaytonaApiError> {
        let mut call = self.toolbox(Method::POST, id, "process/execute").await?.json(request);
        if let Some(seconds) = request.timeout {
            call = call.timeout(self.timeout.max(Duration::from_secs(seconds) + TOOLBOX_DEADLINE_SLACK));
        }
        let bytes = Self::send(call).await?;
        Self::parse(&bytes)
    }

    /// The content of a file; a missing file is [`DaytonaApiError::NotFound`].
    pub async fn download(&self, id: &str, path: &str) -> Result<Bytes, DaytonaApiError> {
        let call = self.toolbox(Method::GET, id, "files/download").await?;
        Self::send(call.query(&[("path", path)])).await
    }

    pub async fn upload(&self, id: &str, path: &str, content: &[u8]) -> Result<(), DaytonaApiError> {
        let (boundary, body) = file_part(path, content);
        let call = self.toolbox(Method::POST, id, "files/bulk-upload").await?;
        Self::send(
            call.header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(body),
        )
        .await
        .map(|_| ())
    }

    /// Removes a file; a file that is already gone is not an error.
    pub async fn delete_file(&self, id: &str, path: &str) -> Result<(), DaytonaApiError> {
        let call = self.toolbox(Method::DELETE, id, "files").await?;
        match Self::send(call.query(&[("path", path)])).await {
            Ok(_) | Err(DaytonaApiError::NotFound(_)) => Ok(()),
            Err(error) => Err(error),
        }
    }

    pub async fn set_permissions(&self, id: &str, path: &str, mode: u32) -> Result<(), DaytonaApiError> {
        let call = self.toolbox(Method::POST, id, "files/permissions").await?;
        Self::send(call.query(&[("path", path), ("mode", &format!("{mode:o}"))]))
            .await
            .map(|_| ())
    }

    /// Creates `path` and its parents with the given octal `mode`.
    pub async fn create_folder(&self, id: &str, path: &str, mode: u32) -> Result<(), DaytonaApiError> {
        let call = self.toolbox(Method::POST, id, "files/folder").await?;
        Self::send(call.query(&[("path", path), ("mode", &format!("{mode:o}"))]))
            .await
            .map(|_| ())
    }

    pub async fn git_clone(&self, id: &str, request: &GitCloneRequest) -> Result<(), DaytonaApiError> {
        let call = self.toolbox(Method::POST, id, "git/clone").await?;
        Self::send(call.json(request)).await.map(|_| ())
    }
}
