use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Duration;

use trogon_credential_domain::{CredentialIdentity, CredentialRef, CredentialScope};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_secret_store::{
    MockOpenBaoSecretStore, SecretMaterial, SecretStorePut, SecretStoreRevoke, SecretStoreRotate,
};
use trogon_webhooks::{
    AuthenticationMode, ConfigurationRevision, EndpointId, EndpointRegistration, EndpointStatus, ProjectId,
    TimestampTolerance, WebhookBodySize,
};

use crate::credential_runtime_projection::{
    InMemoryRuntimeProjectionRepository, RuntimeIntegrationProjection, RuntimeIntegrationStatus,
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
            CredentialIdentity::derive(
                CredentialScope::integration(
                    CredentialOwnerId::new("project-one").unwrap(),
                    SourceKind::Webhook,
                    SourceIntegrationId::new(ENDPOINT).unwrap(),
                ),
                CredentialKind::WebhookSecret,
            ),
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

#[derive(Clone)]
struct UnavailableSecrets;

impl SecretStoreGet for UnavailableSecrets {
    type Error = SecretStoreError;

    async fn get(&self, _credential: &CredentialRef) -> Result<SecretMaterial, Self::Error> {
        Err(SecretStoreError::BackendUnavailable {
            message: "test backend unavailable".to_owned(),
        })
    }
}

#[tokio::test]
async fn generic_webhooks_credential_backend_failure_is_unavailable_not_an_authentication_rejection() {
    let store = MockOpenBaoSecretStore::default();
    let credential = provision_secret(&store).await;
    let projections = InMemoryRuntimeProjectionRepository::default();
    projections
        .upsert(projection(credential, RuntimeIntegrationStatus::Active))
        .await;
    let lookup = RuntimeWebhookSecrets::new(RuntimeCredentialResolver::new(projections, UnavailableSecrets));

    assert!(matches!(
        lookup.lookup_secret(&endpoint("project-one")).await,
        Err(AuthenticationError::Unavailable)
    ));
}

#[derive(Clone, Copy)]
enum ContendedRegistryCreate {
    AlreadyExists,
    Unavailable,
}

#[derive(Clone)]
struct ContendedRegistryClient {
    existing: Option<kv::Store>,
    create: ContendedRegistryCreate,
    lookups: Arc<AtomicUsize>,
}

fn registry_api_error(code: u16, error_code: u64, description: &str) -> async_nats::jetstream::Error {
    serde_json::from_value(serde_json::json!({
        "code": code, "err_code": error_code, "description": description,
    }))
    .unwrap()
}

impl JetStreamGetKeyValue for ContendedRegistryClient {
    type Store = kv::Store;

    async fn get_key_value<T: Into<String> + Send>(&self, _bucket: T) -> Result<Self::Store, context::KeyValueError> {
        if self.lookups.fetch_add(1, Ordering::SeqCst) == 0 {
            return Err(context::KeyValueError::with_source(
                context::KeyValueErrorKind::GetBucket,
                context::GetStreamError::new(context::GetStreamErrorKind::JetStream(registry_api_error(
                    404,
                    10059,
                    "stream not found",
                ))),
            ));
        }
        self.existing
            .clone()
            .ok_or_else(|| context::KeyValueError::new(context::KeyValueErrorKind::GetBucket))
    }
}

impl JetStreamCreateKeyValue for ContendedRegistryClient {
    type Store = kv::Store;

    async fn create_key_value(&self, _config: kv::Config) -> Result<Self::Store, context::CreateKeyValueError> {
        Err(match self.create {
            ContendedRegistryCreate::AlreadyExists => context::CreateKeyValueError::with_source(
                context::CreateKeyValueErrorKind::BucketCreate,
                context::CreateStreamError::new(context::CreateStreamErrorKind::JetStream(registry_api_error(
                    400,
                    10058,
                    "stream name already in use",
                ))),
            ),
            ContendedRegistryCreate::Unavailable => {
                context::CreateKeyValueError::new(context::CreateKeyValueErrorKind::BucketCreate)
            }
        })
    }
}

#[tokio::test]
async fn generic_webhooks_concurrent_registry_creation_reopens_the_winner_without_losing_records() {
    let server = JetStreamTestServer::start().await;
    let context = server.jetstream().await;
    let config = config();
    let existing = open_registry(&context, &config).await.unwrap();
    existing
        .put(
            "existing-registration",
            bytes::Bytes::from_static(b"retained-registration"),
        )
        .await
        .unwrap();
    let lookups = Arc::new(AtomicUsize::new(0));
    let client = ContendedRegistryClient {
        existing: Some(existing),
        create: ContendedRegistryCreate::AlreadyExists,
        lookups: lookups.clone(),
    };

    let reopened = open_registry(&client, &config).await.unwrap();

    assert_eq!(
        reopened.get("existing-registration").await.unwrap().unwrap(),
        bytes::Bytes::from_static(b"retained-registration")
    );
    assert_eq!(lookups.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn generic_webhooks_registry_creation_and_reopen_failures_are_not_treated_as_empty_storage() {
    for (create, expected_lookups) in [
        (ContendedRegistryCreate::Unavailable, 1),
        (ContendedRegistryCreate::AlreadyExists, 2),
    ] {
        let lookups = Arc::new(AtomicUsize::new(0));
        let client = ContendedRegistryClient {
            existing: None,
            create,
            lookups: lookups.clone(),
        };
        let result = open_registry(&client, &config()).await;
        match create {
            ContendedRegistryCreate::Unavailable => assert!(matches!(result, Err(RegistryOpenError::Create(_)))),
            ContendedRegistryCreate::AlreadyExists => assert!(matches!(result, Err(RegistryOpenError::Open(_)))),
        }
        assert_eq!(lookups.load(Ordering::SeqCst), expected_lookups);
    }
}

#[tokio::test]
async fn generic_webhooks_unreadable_registry_is_not_recreated() {
    let client = trogon_nats::jetstream::MockJetStreamKvClient::new();
    client.fail_get(context::KeyValueErrorKind::GetBucket);

    assert!(matches!(
        open_registry(&client, &config()).await,
        Err(RegistryOpenError::Open(_))
    ));
    assert!(client.create_configs().is_empty());
}

#[tokio::test]
async fn generic_webhooks_registry_inspection_failure_stops_opening() {
    let client = trogon_nats::jetstream::MockJetStreamKvClient::new();
    let store = trogon_nats::jetstream::MockJetStreamKvStore::new();
    store.fail_status(kv::StatusErrorKind::TimedOut);
    client.set_get_result(store);

    assert!(matches!(
        open_registry(&client, &config()).await,
        Err(RegistryOpenError::Status(_))
    ));
    assert!(client.create_configs().is_empty());
}
