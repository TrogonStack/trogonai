//! Abstraction over "fetch a short-lived credential from the broker" so
//! [`crate::http_intercept`] can be driven either by a real
//! [`trogon_sandbox_credential_client::CredentialClient`] (one persistent NATS
//! connection, constructed once per process) or by an in-memory test double,
//! without the proxy's connection-handling code depending on NATS directly.

use std::collections::HashMap;
use std::num::NonZeroU32;

use async_trait::async_trait;
use trogon_sandbox_contract::{
    AwsCredentialPairs, AwsCredentialPairsError, CredentialProvider, CredentialRequest, EgressViolationReason, Extract,
    PolicyFence, SecretBindings, SecretMode, SecretName, ShortLivedToken, TenantId,
};
use trogon_sandbox_credential_client::{ClientError, CredentialClient, IssueParams};
use trogon_sandbox_protocol::{
    JwtMaskError, NatsConnectError, RealValue, SentinelRegistry, UnpairedReason, conventional_aws_pair, nats_connect,
};

#[derive(Debug, thiserror::Error)]
pub enum CredentialSourceError {
    #[error("credential broker error: {0}")]
    Broker(#[from] ClientError),
    #[error("no credential broker connection configured for provider {0:?}")]
    UnconfiguredProvider(CredentialProvider),
    #[error("credential request asks for a token that lives zero seconds")]
    ZeroTtl,
}

/// Fetches a [`ShortLivedToken`] for a [`CredentialRequest`] carried on a
/// matched `Transform` domain rule. Implementations must never log the
/// returned token's raw value.
#[async_trait]
pub trait CredentialSource: Send + Sync {
    async fn fetch(&self, request: &CredentialRequest) -> Result<ShortLivedToken, CredentialSourceError>;
}

#[derive(Debug, thiserror::Error)]
pub enum SecretLoadError {
    #[error("secret binding '{name}' could not be fetched: {source}")]
    Fetch {
        name: SecretName,
        source: CredentialSourceError,
    },
    #[error("secret binding '{0}' resolved to an empty value")]
    EmptyValue(SecretName),
    #[error("secret binding '{name}' cannot be masked as a JWT: {source}")]
    Jwt { name: SecretName, source: JwtMaskError },
    #[error("declared AWS credential pair is invalid: {0}")]
    AwsPair(AwsCredentialPairsError),
}

impl SecretLoadError {
    /// The violation reason for a value that could not be masked as the
    /// binding declared, when there is one.
    pub fn violation_reason(&self) -> Option<EgressViolationReason> {
        match self {
            Self::Jwt { source, .. } => Some(source.violation_reason()),
            Self::Fetch { .. } | Self::EmptyValue(_) | Self::AwsPair(_) => None,
        }
    }
}

/// Fetches the real value of every `Mask` binding through `source` and
/// registers it, minting the sentinel the sandbox will see. A value that is
/// empty is refused here; one a header cannot carry is refused when a request
/// would put it in a header. A JWT binding whose value does
/// not verify as one, or names claims it lacks, is refused too.
pub async fn load_sentinel_registry(
    bindings: &SecretBindings,
    source: &dyn CredentialSource,
) -> Result<SentinelRegistry, SecretLoadError> {
    load_sentinel_registry_with_pairs(bindings, &AwsCredentialPairs::default(), source).await
}

/// [`load_sentinel_registry`] with AWS credential pairs the caller declares,
/// for variables that do not carry the conventional names. A declared pair
/// that claims a conventional variable replaces the conventional pairing.
pub async fn load_sentinel_registry_with_pairs(
    bindings: &SecretBindings,
    declared: &AwsCredentialPairs,
    source: &dyn CredentialSource,
) -> Result<SentinelRegistry, SecretLoadError> {
    let mut registry = SentinelRegistry::new();
    for binding in bindings.bindings().iter().filter(|b| b.mode() == SecretMode::Mask) {
        let token = source
            .fetch(binding.source())
            .await
            .map_err(|source| SecretLoadError::Fetch {
                name: binding.name().clone(),
                source,
            })?;
        let real = RealValue::from(token);
        if real.expose().is_empty() {
            return Err(SecretLoadError::EmptyValue(binding.name().clone()));
        }
        match binding.extract() {
            Some(Extract::Jwt(masking)) => {
                registry
                    .register_jwt(binding.name().clone(), masking, real, binding.inject_hosts().clone())
                    .map_err(|source| SecretLoadError::Jwt {
                        name: binding.name().clone(),
                        source,
                    })?;
            }
            _ => {
                registry.register(binding.name().clone(), real, binding.inject_hosts().clone());
            }
        }
        registry.set_scope(binding.name(), binding.inject_scope());
        registry.register_strip(binding.inject_hosts().clone(), binding.strip_response_headers().clone());
    }
    let conventional = conventional_aws_pair(bindings, declared);
    for (name, reason) in &conventional.unpaired {
        match reason {
            UnpairedReason::SecretAccessKeyWithoutAccessKeyId => tracing::warn!(
                binding = %name,
                "AWS secret access key is masked but AWS_ACCESS_KEY_ID is not, so SigV4 requests cannot be re-signed"
            ),
            UnpairedReason::HostsDifferFromAccessKeyId => tracing::warn!(
                binding = %name,
                "AWS credential injects at different hosts than AWS_ACCESS_KEY_ID and is left out of the pair"
            ),
        }
    }
    for pair in conventional.pair.iter().chain(declared.pairs()) {
        registry.link_aws_pair(pair).map_err(SecretLoadError::AwsPair)?;
    }
    Ok(registry)
}

/// Real [`CredentialSource`] backed by one [`CredentialClient`] per
/// [`CredentialProvider`], all sharing one NATS connection. `CredentialClient`
/// is scoped to one `(provider, tenant)` pair, so this holds one client per
/// provider actually referenced by the sandbox's `NetworkPolicy`, all sharing
/// the one tenant the proxy process serves; the connection is made once at
/// proxy startup and reused for the sandbox's lifetime rather than
/// reconnecting per-connection.
pub struct BrokerCredentialSource {
    clients: HashMap<CredentialProvider, CredentialClient<async_nats::Client>>,
}

impl BrokerCredentialSource {
    /// Connects to `nats_url` and builds one [`CredentialClient`] per entry in
    /// `providers` for `tenant`, failing fast if the connection cannot be
    /// established.
    pub async fn connect(
        nats_url: &str,
        tenant: TenantId,
        providers: &[CredentialProvider],
    ) -> Result<Self, NatsConnectError> {
        let connection = nats_connect::connect(nats_url).await?;
        Ok(Self::over(connection, tenant, providers, None))
    }

    /// [`Self::connect`] with every client stamping `fence` (the lease the
    /// proxy serves and the committed policy revision it was started from) on
    /// each request, so the broker resolves only references that revision
    /// names for this tenant.
    pub async fn connect_fenced(
        nats_url: &str,
        tenant: TenantId,
        providers: &[CredentialProvider],
        fence: PolicyFence,
    ) -> Result<Self, NatsConnectError> {
        let connection = nats_connect::connect(nats_url).await?;
        Ok(Self::over(connection, tenant, providers, Some(fence)))
    }

    fn over(
        connection: async_nats::Client,
        tenant: TenantId,
        providers: &[CredentialProvider],
        fence: Option<PolicyFence>,
    ) -> Self {
        let clients = providers
            .iter()
            .map(|provider| {
                let client = CredentialClient::new(connection.clone(), *provider, tenant.clone());
                let client = match &fence {
                    Some(fence) => client.with_policy_fence(fence.clone()),
                    None => client,
                };
                (*provider, client)
            })
            .collect();
        Self { clients }
    }
}

#[async_trait]
impl CredentialSource for BrokerCredentialSource {
    async fn fetch(&self, request: &CredentialRequest) -> Result<ShortLivedToken, CredentialSourceError> {
        let client = self
            .clients
            .get(&request.provider)
            .ok_or(CredentialSourceError::UnconfiguredProvider(request.provider))?;
        let ttl_seconds = NonZeroU32::new(request.ttl_seconds).ok_or(CredentialSourceError::ZeroTtl)?;
        let response = client
            .issue(IssueParams {
                pull_secret_id: request.pull_secret_id.clone(),
                scope: request.scope.clone(),
                ttl_seconds,
                requester: request.requester.clone(),
            })
            .await?;
        Ok(response.token)
    }
}

/// [`CredentialSource`] used when no credential broker connection is
/// configured for the process (e.g. no `Transform` domain rules in the
/// loaded policy). Errors on every request rather than silently forwarding
/// requests without their intended credential, so a policy that later gains
/// a `Transform` rule without the process being reconfigured fails loudly
/// instead of leaking unauthenticated requests upstream.
#[derive(Debug, Default)]
pub struct UnconfiguredCredentialSource;

#[async_trait]
impl CredentialSource for UnconfiguredCredentialSource {
    async fn fetch(&self, request: &CredentialRequest) -> Result<ShortLivedToken, CredentialSourceError> {
        Err(CredentialSourceError::UnconfiguredProvider(request.provider))
    }
}

#[cfg(test)]
pub(crate) mod tests;
