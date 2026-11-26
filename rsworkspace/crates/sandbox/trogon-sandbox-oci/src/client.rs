use std::time::Duration;

use oci_client::Reference;
use oci_client::client::{Client, ClientConfig, ClientProtocol};
use oci_client::secrets::RegistryAuth;
use tokio::time::Instant;
use tracing::warn;

use crate::auth::AuthResolver;
use crate::transport::BoundedRegistry;
use crate::{OciClientConfig, OciError, OciMetrics, RegistryHost};

const METADATA_DEADLINE: Duration = Duration::from_secs(120);

/// A digest-verifying client for OCI registries.
///
/// Push, pull, referrer, readiness, container-image, and tag operations are implemented in
/// separate modules and share this handle.
pub struct OciClient {
    inner: Client,
    auth: AuthResolver,
    pub(crate) metrics: Option<OciMetrics>,
    pub(crate) http: reqwest::Client,
    insecure_registries: Vec<RegistryHost>,
}

impl OciClient {
    /// A client that falls back to anonymous access when stored Docker credentials cannot be
    /// read, logging the reason.
    pub fn new(config: OciClientConfig) -> Self {
        Self::with_metrics(config, None)
    }

    /// Like [`Self::new`], recording transfer metrics when `metrics` is set.
    pub fn with_metrics(config: OciClientConfig, metrics: Option<OciMetrics>) -> Self {
        let auth = AuthResolver::new(&config).unwrap_or_else(|error| {
            warn!(%error, "ignoring unreadable docker credentials");
            AuthResolver::default()
        });
        Self::assemble(config, auth, metrics)
    }

    /// A client that fails when stored Docker credentials cannot be read.
    pub fn try_new(config: OciClientConfig) -> Result<Self, OciError> {
        let auth = AuthResolver::new(&config)?;
        Ok(Self::assemble(config, auth, None))
    }

    fn assemble(config: OciClientConfig, auth: AuthResolver, metrics: Option<OciMetrics>) -> Self {
        let protocol = if config.insecure_registries.is_empty() {
            ClientProtocol::Https
        } else {
            ClientProtocol::HttpsExcept(config.insecure_registries.iter().map(ToString::to_string).collect())
        };
        let inner = Client::new(ClientConfig {
            protocol,
            ..Default::default()
        });
        Self {
            inner,
            auth,
            metrics,
            http: reqwest::Client::new(),
            insecure_registries: config.insecure_registries,
        }
    }

    pub(crate) fn transport(
        &self,
        reference: &Reference,
        auth: RegistryAuth,
        deadline: Instant,
    ) -> Result<BoundedRegistry, OciError> {
        BoundedRegistry::new(reference, auth, &self.insecure_registries, deadline)
    }

    pub(crate) fn inner(&self) -> &Client {
        &self.inner
    }

    pub(crate) fn auth(&self) -> &AuthResolver {
        &self.auth
    }

    pub(crate) fn is_insecure(&self, registry: &str) -> bool {
        self.insecure_registries
            .iter()
            .any(|insecure| insecure.as_str() == registry)
    }
}

pub(crate) fn metadata_deadline() -> Instant {
    Instant::now() + METADATA_DEADLINE
}
