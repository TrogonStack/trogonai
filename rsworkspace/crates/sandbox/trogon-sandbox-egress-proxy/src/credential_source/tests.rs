use super::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    CredentialError, CredentialScope, DomainPattern, EnvVarName, Host, InjectHosts, LeaseId, PullSecretId,
    ResponseHeaderStrip, SecretBinding, SecretBindingParts, SecretTarget, StrippedHeader,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// In-memory [`CredentialSource`] double keyed by `pull_secret_id`, so
/// tests can assert exactly which credential a matched request received
/// without a live NATS broker or credential-broker process.
#[derive(Default)]
pub(crate) struct FakeCredentialSource {
    tokens: HashMap<String, ShortLivedToken>,
    fetch_count: AtomicU32,
}

impl FakeCredentialSource {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn with_token(mut self, pull_secret_id: &str, token: &str) -> Self {
        self.tokens
            .insert(pull_secret_id.to_string(), ShortLivedToken::new(token));
        self
    }

    #[allow(dead_code)]
    pub(crate) fn fetch_count(&self) -> u32 {
        self.fetch_count.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl CredentialSource for FakeCredentialSource {
    async fn fetch(&self, request: &CredentialRequest) -> Result<ShortLivedToken, CredentialSourceError> {
        self.fetch_count.fetch_add(1, Ordering::Relaxed);
        self.tokens
            .get(request.pull_secret_id.as_str())
            .cloned()
            .ok_or_else(|| {
                CredentialSourceError::Broker(ClientError::Broker(CredentialError::NotFound {
                    message: format!("no fake token configured for pull_secret_id {}", request.pull_secret_id),
                }))
            })
    }
}

fn test_credential_request(provider: CredentialProvider) -> CredentialRequest {
    CredentialRequest {
        provider,
        pull_secret_id: PullSecretId::new("test-secret"),
        scope: CredentialScope::RegistryPull {
            repository_glob: "*".to_string(),
        },
        ttl_seconds: 60,
        requester: None,
    }
}

#[tokio::test]
async fn broker_source_errors_on_unconfigured_provider() -> TestResult {
    let source = BrokerCredentialSource {
        clients: HashMap::new(),
    };
    let request = test_credential_request(CredentialProvider::Registry);

    let result = source.fetch(&request).await;

    match result {
        Err(CredentialSourceError::UnconfiguredProvider(provider)) => {
            assert_eq!(provider, CredentialProvider::Registry);
            Ok(())
        }
        other => Err(format!("expected UnconfiguredProvider error, got {other:?}").into()),
    }
}

#[tokio::test]
async fn broker_source_reports_the_specific_requested_provider() -> TestResult {
    let source = BrokerCredentialSource {
        clients: HashMap::new(),
    };
    let request = test_credential_request(CredentialProvider::S3);

    let result = source.fetch(&request).await;

    match result {
        Err(CredentialSourceError::UnconfiguredProvider(provider)) => {
            assert_eq!(provider, CredentialProvider::S3);
            Ok(())
        }
        other => Err(format!("expected UnconfiguredProvider error, got {other:?}").into()),
    }
}

fn env_binding(var: &str, secret_id: &str, hosts: &[&str]) -> Result<SecretBinding, Box<dyn std::error::Error>> {
    let mut source = test_credential_request(CredentialProvider::Registry);
    source.pull_secret_id = PullSecretId::new(secret_id);
    let patterns = hosts
        .iter()
        .map(|h| h.parse::<DomainPattern>())
        .collect::<Result<Vec<_>, _>>()?;
    Ok(SecretBinding::new(SecretBindingParts {
        name: SecretName::new(var.to_lowercase().replace('_', "-"))?,
        target: SecretTarget::Env(EnvVarName::new(var)?),
        mode: SecretMode::Mask,
        extract: None,
        mask_duplicates: Default::default(),
        on_extract_no_match: Default::default(),
        strip_response_headers: Default::default(),
        inject_hosts: InjectHosts::new(patterns)?,
        inject_scope: Default::default(),
        source,
    })?)
}

fn aws_source() -> FakeCredentialSource {
    FakeCredentialSource::new()
        .with_token("akid", "AKIAREALKEYID0000001")
        .with_token("secret", "realSecretAccessKeyValue0123456789abcdefgh")
        .with_token("token", "realSessionTokenValue")
}

#[tokio::test]
async fn the_conventional_aws_trio_is_linked_without_declaration() -> TestResult {
    let bindings = SecretBindings::new(vec![
        env_binding("AWS_ACCESS_KEY_ID", "akid", &["s3.amazonaws.com"])?,
        env_binding("AWS_SECRET_ACCESS_KEY", "secret", &["s3.amazonaws.com"])?,
        env_binding("AWS_SESSION_TOKEN", "token", &["s3.amazonaws.com"])?,
    ])?;
    let registry = load_sentinel_registry(&bindings, &aws_source()).await?;
    let host: Host = "s3.amazonaws.com".parse()?;
    let pairs: Vec<_> = registry.aws_pairs_for_host(&host, 443).collect();
    assert_eq!(pairs.len(), 1);
    assert_eq!(pairs[0].access_key_id.expose(), "AKIAREALKEYID0000001");
    assert_eq!(
        pairs[0].session_token.map(|t| t.expose().to_string()),
        Some("realSessionTokenValue".to_string())
    );
    Ok(())
}

#[tokio::test]
async fn a_lone_secret_access_key_is_not_linked() -> TestResult {
    let bindings = SecretBindings::new(vec![env_binding(
        "AWS_SECRET_ACCESS_KEY",
        "secret",
        &["s3.amazonaws.com"],
    )?])?;
    let registry = load_sentinel_registry(&bindings, &aws_source()).await?;
    let host: Host = "s3.amazonaws.com".parse()?;
    assert_eq!(registry.aws_pairs_for_host(&host, 443).count(), 0);
    Ok(())
}

#[tokio::test]
async fn strip_rules_follow_the_binding_inject_hosts() -> TestResult {
    let plain = env_binding("API_TOKEN", "akid", &["api.example.com"])?;
    let binding = SecretBinding::new(SecretBindingParts {
        name: plain.name().clone(),
        target: plain.target().clone(),
        mode: plain.mode(),
        extract: None,
        mask_duplicates: plain.mask_duplicates(),
        on_extract_no_match: plain.on_extract_no_match(),
        strip_response_headers: ResponseHeaderStrip::new([StrippedHeader::new("set-cookie")?]),
        inject_hosts: plain.inject_hosts().clone(),
        inject_scope: plain.inject_scope().clone(),
        source: plain.source().clone(),
    })?;
    let registry = load_sentinel_registry(&SecretBindings::new(vec![binding])?, &aws_source()).await?;
    let on: Host = "api.example.com".parse()?;
    let off: Host = "other.example.com".parse()?;
    assert!(
        registry
            .strip_for_host(&on, 443)
            .headers()
            .contains(&StrippedHeader::new("Set-Cookie")?)
    );
    assert!(registry.strip_for_host(&off, 443).is_empty());
    Ok(())
}

#[test]
fn only_a_jwt_failure_carries_a_violation_reason() -> TestResult {
    let name = SecretName::new("api-token")?;
    let jwt = SecretLoadError::Jwt {
        name: name.clone(),
        source: JwtMaskError::NoClaimMatched,
    };
    assert!(jwt.violation_reason().is_some());
    assert!(SecretLoadError::EmptyValue(name.clone()).violation_reason().is_none());
    let fetch = SecretLoadError::Fetch {
        name,
        source: CredentialSourceError::ZeroTtl,
    };
    assert!(fetch.violation_reason().is_none());
    Ok(())
}

#[tokio::test]
async fn an_access_key_at_other_hosts_is_reported_and_left_out_of_the_pair() -> TestResult {
    let bindings = SecretBindings::new(vec![
        env_binding("AWS_ACCESS_KEY_ID", "akid", &["s3.amazonaws.com"])?,
        env_binding("AWS_SECRET_ACCESS_KEY", "secret", &["sts.amazonaws.com"])?,
    ])?;
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(std::io::sink)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let registry = load_sentinel_registry(&bindings, &aws_source()).await?;

    let host: Host = "s3.amazonaws.com".parse()?;
    assert_eq!(registry.aws_pairs_for_host(&host, 443).count(), 0);
    Ok(())
}

#[tokio::test]
async fn a_lone_secret_access_key_is_reported_when_logging_is_enabled() -> TestResult {
    let bindings = SecretBindings::new(vec![env_binding(
        "AWS_SECRET_ACCESS_KEY",
        "secret",
        &["s3.amazonaws.com"],
    )?])?;
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(std::io::sink)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let registry = load_sentinel_registry(&bindings, &aws_source()).await?;

    assert!(!registry.is_empty());
    Ok(())
}

#[tokio::test]
async fn the_unconfigured_source_refuses_every_request() -> TestResult {
    let result = UnconfiguredCredentialSource
        .fetch(&test_credential_request(CredentialProvider::S3))
        .await;
    assert!(matches!(
        result,
        Err(CredentialSourceError::UnconfiguredProvider(CredentialProvider::S3))
    ));
    Ok(())
}

#[tokio::test]
async fn the_broker_source_connects_with_and_without_a_fence() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let url = format!("nats://{}", server.address());
    let tenant = TenantId::new("tenant-one")?;
    let providers = [CredentialProvider::Registry, CredentialProvider::S3];

    let plain = BrokerCredentialSource::connect(&url, tenant.clone(), &providers).await?;
    let fenced = BrokerCredentialSource::connect_fenced(
        &url,
        tenant,
        &providers,
        PolicyFence {
            lease_id: LeaseId::new("lease-one"),
            policy_revision: 3,
        },
    )
    .await?;

    assert_eq!(plain.clients.len(), 2);
    assert_eq!(fenced.clients.len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_zero_lifetime_request_is_refused_before_reaching_the_broker() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let url = format!("nats://{}", server.address());
    let source =
        BrokerCredentialSource::connect(&url, TenantId::new("tenant-one")?, &[CredentialProvider::Registry]).await?;
    let mut request = test_credential_request(CredentialProvider::Registry);
    request.ttl_seconds = 0;

    let result = source.fetch(&request).await;

    assert!(matches!(result, Err(CredentialSourceError::ZeroTtl)));
    Ok(())
}
