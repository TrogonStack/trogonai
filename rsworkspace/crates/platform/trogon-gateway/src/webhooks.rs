pub(crate) mod config;
mod registry_bucket;

use async_nats::jetstream::{context, kv};
use trogon_credential_domain::{
    CredentialKind, CredentialOwnerId, RuntimeReleaseRequest, RuntimeServiceId, SourceIntegrationId, SourceKind,
};
use trogon_nats::jetstream::{
    JetStreamContext, JetStreamCreateKeyValue, JetStreamGetKeyValue, JetStreamKeyValueStatus, ProvisionedStreamField,
    is_create_key_value_already_exists, is_get_key_value_not_found,
};
use trogon_secret_store::{SecretStoreError, SecretStoreGet};
use trogon_std::SecretString;
use trogon_webhooks::{AuthenticationError, RegisteredEndpoint, WebhookSecretLookup};

use crate::credential_runtime_projection::{RuntimeCredentialError, RuntimeCredentialResolver, RuntimeIntegrationKey};

pub use config::{WebhookConfig, WebhookConfigInput};

pub async fn provision<C: JetStreamContext>(client: &C, config: &WebhookConfig) -> Result<(), C::Error> {
    client
        .create_or_reconcile_stream(
            async_nats::jetstream::stream::Config {
                name: config.stream_name.as_str().to_owned(),
                subjects: vec![format!("{}.>", config.subject_prefix)],
                max_age: config.stream_max_age.into(),
                duplicate_window: config.duplicate_window.into(),
                ..Default::default()
            },
            &[
                ProvisionedStreamField::Subjects,
                ProvisionedStreamField::MaxAge,
                ProvisionedStreamField::DuplicateWindow,
            ],
        )
        .await
}

#[derive(Debug, thiserror::Error)]
pub enum RegistryOpenError {
    #[error("failed to open webhook endpoint registry")]
    Open(#[source] context::KeyValueError),
    #[error("failed to create webhook endpoint registry")]
    Create(#[source] context::CreateKeyValueError),
    #[error("failed to inspect webhook endpoint registry")]
    Status(#[source] kv::StatusError),
    #[error("webhook endpoint registry has incompatible {field}")]
    Configuration { field: RegistryConfigurationField },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryConfigurationField {
    BucketName,
    Retention,
    History,
    Subjects,
    Storage,
    ReplicationSources,
    MessageTtl,
    DeleteMarkerTtl,
}

impl std::fmt::Display for RegistryConfigurationField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::BucketName => "bucket name",
            Self::Retention => "retention",
            Self::History => "history",
            Self::Subjects => "subjects",
            Self::Storage => "storage",
            Self::ReplicationSources => "replication sources",
            Self::MessageTtl => "message TTL",
            Self::DeleteMarkerTtl => "delete marker TTL",
        })
    }
}

pub async fn open_registry<C>(
    client: &C,
    config: &WebhookConfig,
) -> Result<<C as JetStreamGetKeyValue>::Store, RegistryOpenError>
where
    C: JetStreamGetKeyValue + JetStreamCreateKeyValue<Store = <C as JetStreamGetKeyValue>::Store>,
{
    let store = match client.get_key_value(config.registry_bucket.as_str()).await {
        Ok(store) => store,
        Err(source) if is_get_key_value_not_found(&source) => {
            match client
                .create_key_value(kv::Config {
                    bucket: config.registry_bucket.as_str().to_owned(),
                    history: 1,
                    max_value_size: 4096,
                    ..Default::default()
                })
                .await
            {
                Ok(store) => store,
                Err(source) if is_create_key_value_already_exists(&source) => client
                    .get_key_value(config.registry_bucket.as_str())
                    .await
                    .map_err(RegistryOpenError::Open)?,
                Err(source) => return Err(RegistryOpenError::Create(source)),
            }
        }
        Err(source) => return Err(RegistryOpenError::Open(source)),
    };
    let status = store.status().await.map_err(RegistryOpenError::Status)?;
    let stream = &status.info.config;
    let expected_subject = format!("$KV.{}.>", config.registry_bucket);
    for (valid, field) in [
        (
            status.bucket() == config.registry_bucket.as_str(),
            RegistryConfigurationField::BucketName,
        ),
        (status.max_age().is_zero(), RegistryConfigurationField::Retention),
        (status.history() == 1, RegistryConfigurationField::History),
        (
            stream.subjects == [expected_subject],
            RegistryConfigurationField::Subjects,
        ),
        (
            stream.storage == async_nats::jetstream::stream::StorageType::File,
            RegistryConfigurationField::Storage,
        ),
        (
            stream.mirror.is_none() && stream.sources.is_none(),
            RegistryConfigurationField::ReplicationSources,
        ),
        (!stream.allow_message_ttl, RegistryConfigurationField::MessageTtl),
        (
            stream.subject_delete_marker_ttl.is_none(),
            RegistryConfigurationField::DeleteMarkerTtl,
        ),
    ] {
        if !valid {
            return Err(RegistryOpenError::Configuration { field });
        }
    }
    Ok(store)
}

#[derive(Clone)]
pub struct RuntimeWebhookSecrets<G> {
    resolver: RuntimeCredentialResolver<G>,
}

impl<G> RuntimeWebhookSecrets<G> {
    pub fn new(resolver: RuntimeCredentialResolver<G>) -> Self {
        Self { resolver }
    }
}

impl<G> WebhookSecretLookup for RuntimeWebhookSecrets<G>
where
    G: SecretStoreGet<Error = SecretStoreError>,
{
    async fn lookup_secret(&self, endpoint: &RegisteredEndpoint) -> Result<SecretString, AuthenticationError> {
        let registration = endpoint.registration();
        let integration = SourceIntegrationId::new(registration.endpoint_id().as_str())
            .map_err(|_| AuthenticationError::Unauthorized)?;
        let owner = CredentialOwnerId::new(registration.project_id().as_str())
            .map_err(|_| AuthenticationError::Unauthorized)?;
        let service = RuntimeServiceId::new("trogon-gateway").map_err(|_| AuthenticationError::Unavailable)?;
        let key = RuntimeIntegrationKey::new(SourceKind::Webhook, &integration);
        self.resolver
            .resolve_plaintext_for_owner(
                &key,
                CredentialKind::WebhookSecret,
                &owner,
                &RuntimeReleaseRequest::new().by_runtime_service(&service),
            )
            .await
            .map_err(|error| match error {
                error @ RuntimeCredentialError::SecretStore(SecretStoreError::BackendUnavailable { .. }) => {
                    AuthenticationError::UnavailableWithSource(Box::new(error))
                }
                _ => AuthenticationError::Unauthorized,
            })
    }
}

#[cfg(test)]
mod tests;
