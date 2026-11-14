use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use trogon_nats::test_support::JetStreamTestServer;
use trogon_webhooks::{
    AuthenticationMode, ConfigurationRevision, EndpointId, EndpointRegistration, EndpointStatus, ProjectId,
    TimestampTolerance, WebhookBodySize,
};

use crate::credential::commands::domain::{CredentialRef, CredentialScope};
use crate::credential_runtime_projection::{
    InMemoryRuntimeProjectionRepository, RuntimeIntegrationProjection, RuntimeIntegrationStatus,
};
use crate::secret_store::{
    MockOpenBaoSecretStore, SecretMaterial, SecretStorePut, SecretStoreRevoke, SecretStoreRotate,
};

use super::*;

const ENDPOINT: &str = "3cc8a622-fcdd-42c2-b4ad-88d638053814";

fn endpoint(project: &str) -> RegisteredEndpoint {
    RegisteredEndpoint::new(
        EndpointRegistration::new(
            EndpointId::new(ENDPOINT).unwrap(),
            ProjectId::new(project).unwrap(),
            EndpointStatus::Enabled,
            AuthenticationMode::Bearer,
            WebhookBodySize::new(4096).unwrap(),
            TimestampTolerance::new(60).unwrap(),
        ),
        ConfigurationRevision::new(1).unwrap(),
    )
}

fn projection(credential: CredentialRef, status: RuntimeIntegrationStatus) -> RuntimeIntegrationProjection {
    RuntimeIntegrationProjection::new(
        credential.owner_id().clone(),
        SourceKind::Webhook,
        SourceIntegrationId::new(ENDPOINT).unwrap(),
        status,
        credential.version().get(),
    )
    .with_credential(CredentialKind::WebhookSecret, credential)
}

async fn provision_secret(store: &MockOpenBaoSecretStore) -> CredentialRef {
    store
        .put(
            CredentialScope::integration(
                CredentialOwnerId::new("project-one").unwrap(),
                SourceKind::Webhook,
                SourceIntegrationId::new(ENDPOINT).unwrap(),
            ),
            CredentialKind::WebhookSecret,
            SecretString::new("test-initial-value").unwrap(),
        )
        .await
        .unwrap()
}

#[derive(Clone)]
struct CountingSecrets {
    store: MockOpenBaoSecretStore,
    reads: Arc<AtomicUsize>,
}

impl SecretStoreGet for CountingSecrets {
    type Error = SecretStoreError;

    async fn get(&self, credential: &CredentialRef) -> Result<SecretMaterial, Self::Error> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.store.get(credential).await
    }
}

#[tokio::test]
async fn generic_webhooks_runtime_secret_matches_endpoint_project_and_current_rotation() {
    let store = MockOpenBaoSecretStore::default();
    let first = provision_secret(&store).await;
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections
        .upsert(projection(first.clone(), RuntimeIntegrationStatus::Active))
        .await;
    let resolver = RuntimeCredentialResolver::new(projections.clone(), store.clone());
    let lookup = RuntimeWebhookSecrets::new(resolver);
    let registered = endpoint("project-one");

    assert_eq!(
        lookup.lookup_secret(&registered).await.unwrap().as_str(),
        "test-initial-value"
    );
    let second = store
        .rotate(&first, SecretString::new("test-rotated-value").unwrap())
        .await
        .unwrap();
    projections
        .upsert(projection(second, RuntimeIntegrationStatus::Active))
        .await;
    assert_eq!(
        lookup.lookup_secret(&registered).await.unwrap().as_str(),
        "test-rotated-value"
    );
}

#[tokio::test]
async fn generic_webhooks_foreign_project_cannot_read_or_reuse_a_cached_secret() {
    let store = MockOpenBaoSecretStore::default();
    let credential = provision_secret(&store).await;
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections
        .upsert(projection(credential, RuntimeIntegrationStatus::Active))
        .await;
    let reads = Arc::new(AtomicUsize::new(0));
    let lookup = RuntimeWebhookSecrets::new(RuntimeCredentialResolver::new(
        projections,
        CountingSecrets {
            store,
            reads: reads.clone(),
        },
    ));

    assert!(matches!(
        lookup.lookup_secret(&endpoint("foreign-project")).await,
        Err(AuthenticationError::Unauthorized)
    ));
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    lookup.lookup_secret(&endpoint("project-one")).await.unwrap();
    assert!(matches!(
        lookup.lookup_secret(&endpoint("foreign-project")).await,
        Err(AuthenticationError::Unauthorized)
    ));
    assert_eq!(reads.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn generic_webhooks_disabled_and_revoked_credentials_fail_closed() {
    let store = MockOpenBaoSecretStore::default();
    let credential = provision_secret(&store).await;
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections
        .upsert(projection(credential.clone(), RuntimeIntegrationStatus::Disabled))
        .await;
    let lookup = RuntimeWebhookSecrets::new(RuntimeCredentialResolver::new(projections.clone(), store.clone()));

    assert!(matches!(
        lookup.lookup_secret(&endpoint("project-one")).await,
        Err(AuthenticationError::Unauthorized)
    ));
    store.revoke(&credential).await.unwrap();
    projections
        .upsert(projection(credential, RuntimeIntegrationStatus::Active))
        .await;
    assert!(matches!(
        lookup.lookup_secret(&endpoint("project-one")).await,
        Err(AuthenticationError::Unauthorized)
    ));
}

fn config() -> WebhookConfig {
    let config = crate::config::load(None).unwrap();
    assert!(config.webhooks.is_none());
    WebhookConfig {
        registry_bucket: registry_bucket::RegistryBucket::new("GATEWAY_WEBHOOK_ENDPOINTS".to_owned()).unwrap(),
        stream_name: trogon_nats::NatsToken::new("WEBHOOKS").unwrap(),
        subject_prefix: trogon_nats::DottedNatsToken::new("webhooks").unwrap(),
        stream_max_age: trogon_nats::jetstream::StreamMaxAge::from_secs(604_800).unwrap(),
        duplicate_window: trogon_std::NonZeroDuration::from_secs(604_800).unwrap(),
        admission_timeout: trogon_std::NonZeroDuration::from_secs(10).unwrap(),
    }
}

#[tokio::test]
async fn generic_webhooks_registry_is_provisioned_and_reopened_without_rewriting_operator_limits() {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let config = config();
    let store = open_registry(&context, &config).await.unwrap();
    let status = store.status().await.unwrap();
    assert_eq!(status.history(), 1);
    assert!(status.max_age().is_zero());
    assert_eq!(status.info.config.max_message_size, 4096);

    let mut stream_config = status.info.config;
    stream_config.description = Some("operator-owned-description".to_owned());
    stream_config.max_bytes = 1_000_000;
    context.update_stream(&stream_config).await.unwrap();
    let reopened = open_registry(&context, &config).await.unwrap();
    let status = reopened.status().await.unwrap();
    assert_eq!(
        status.info.config.description.as_deref(),
        Some("operator-owned-description")
    );
    assert_eq!(status.info.config.max_bytes, 1_000_000);
}

#[tokio::test]
async fn generic_webhooks_registry_refuses_expiring_config_without_rewriting_it() {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let config = config();
    context
        .create_key_value(kv::Config {
            bucket: config.registry_bucket.as_str().to_owned(),
            history: 1,
            max_age: Duration::from_secs(60),
            ..Default::default()
        })
        .await
        .unwrap();

    assert!(matches!(
        open_registry(&context, &config).await,
        Err(RegistryOpenError::Configuration { field: "retention" })
    ));
    let unchanged = context.get_key_value(config.registry_bucket.as_str()).await.unwrap();
    assert_eq!(unchanged.status().await.unwrap().max_age(), Duration::from_secs(60));
}
