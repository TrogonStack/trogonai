use trogon_sandbox_contract::{CredentialProviderError, TenantIdError};

/// A credential request could not be served, or the service could not run.
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error("the subject is not a credential request subject")]
    UnexpectedSubject,
    #[error("the subject names no tenant")]
    MissingTenant,
    #[error("the subject names an unknown credential provider")]
    Provider(#[from] CredentialProviderError),
    #[error("the subject names an invalid tenant")]
    Tenant(#[from] TenantIdError),
    #[error("the request payload is not valid")]
    Decode(#[from] serde_json::Error),
    #[error("the reply could not be encoded")]
    Encode(#[source] serde_json::Error),
    #[error("subscribing to the request subjects failed")]
    Subscribe(#[from] async_nats::client::SubscribeError),
    #[error("flushing the subscriptions failed")]
    Flush(#[from] async_nats::client::FlushError),
    #[error("publishing the reply failed")]
    Publish(#[from] async_nats::client::PublishError),
}
