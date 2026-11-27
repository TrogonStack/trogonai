use std::time::Duration;

use reqwest::{Method, RequestBuilder};
use serde::Serialize;
use serde::de::DeserializeOwned;
use url::Url;

use crate::api_wire::{Envelope, decode_response};
use crate::constants::{CONNECT_TIMEOUT, REQUEST_TIMEOUT};
use crate::{ProxmoxApiError, ResourcePoolId};

/// The Proxmox VE REST client, scoped to one node and one API token. It never follows redirects, so the token
/// cannot leave the configured endpoint.
#[derive(Clone)]
pub struct ProxmoxApi {
    pub(crate) base: Url,
    pub(crate) node: String,
    pub(crate) authorization: reqwest::header::HeaderValue,
    pub(crate) http: reqwest::Client,
    pub(crate) task_timeout: Duration,
    pub(crate) pool: Option<ResourcePoolId>,
}

impl ProxmoxApi {
    /// An endpoint must be HTTP(S) without credentials, and HTTPS unless its host is loopback.
    pub fn new(
        endpoint: Url,
        node: String,
        token_id: &str,
        token_secret: &str,
        task_timeout: Duration,
    ) -> Result<Self, ProxmoxApiError> {
        if !matches!(endpoint.scheme(), "http" | "https") {
            return Err(ProxmoxApiError::UnsupportedScheme);
        }
        if endpoint.scheme() != "https" && !matches!(endpoint.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")) {
            return Err(ProxmoxApiError::InsecureEndpoint);
        }
        if !endpoint.username().is_empty() || endpoint.password().is_some() {
            return Err(ProxmoxApiError::CredentialsInUrl);
        }
        if endpoint.cannot_be_a_base() {
            return Err(ProxmoxApiError::UnusableUrl);
        }
        if node.is_empty() {
            return Err(ProxmoxApiError::MissingNode);
        }
        if task_timeout.is_zero() {
            return Err(ProxmoxApiError::ZeroTaskTimeout);
        }
        let mut authorization: reqwest::header::HeaderValue =
            format!("PVEAPIToken={token_id}={token_secret}").parse()?;
        authorization.set_sensitive(true);
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(ProxmoxApiError::Client)?;
        Ok(Self {
            base: endpoint,
            node,
            authorization,
            http,
            task_timeout,
            pool: None,
        })
    }

    /// Clones land in this resource pool.
    pub fn with_pool(mut self, pool: Option<ResourcePoolId>) -> Self {
        self.pool = pool;
        self
    }

    pub(crate) fn url(&self, segments: &[&str]) -> Result<Url, ProxmoxApiError> {
        let mut url = self.base.clone();
        url.set_query(None);
        url.set_fragment(None);
        url.path_segments_mut()
            .map_err(|()| ProxmoxApiError::UnusableUrl)?
            .clear()
            .extend(["api2", "json"])
            .extend(segments.iter().copied());
        Ok(url)
    }

    pub(crate) fn builder(&self, method: Method, url: Url) -> RequestBuilder {
        self.http
            .request(method, url)
            .header(reqwest::header::AUTHORIZATION, self.authorization.clone())
    }

    /// The segments of a path below this API's node.
    pub(crate) fn node_path<'a>(&'a self, rest: &[&'a str]) -> Vec<&'a str> {
        let mut segments = vec!["nodes", self.node.as_str()];
        segments.extend(rest.iter().copied());
        segments
    }

    /// The segments of a path below one of this node's VMs.
    pub(crate) fn vm_path<'a>(&'a self, vm: &'a str, rest: &[&'a str]) -> Vec<&'a str> {
        let mut segments = vec!["nodes", self.node.as_str(), "qemu", vm];
        segments.extend(rest.iter().copied());
        segments
    }

    pub(crate) async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        segments: &[&str],
    ) -> Result<T, ProxmoxApiError> {
        self.send(self.builder(method, self.url(segments)?)).await
    }

    pub(crate) async fn request_with<T: DeserializeOwned>(
        &self,
        method: Method,
        segments: &[&str],
        body: &(impl Serialize + ?Sized),
    ) -> Result<T, ProxmoxApiError> {
        self.send(self.builder(method, self.url(segments)?).json(body)).await
    }

    async fn send<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T, ProxmoxApiError> {
        let response = request.send().await?;
        let status = response.status();
        if !status.is_success() {
            return Err(ProxmoxApiError::Status(status));
        }
        Ok(decode_response::<Envelope<T>>(response).await?.data)
    }
}
