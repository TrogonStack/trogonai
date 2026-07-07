use std::future::poll_fn;
use std::pin::Pin;

use futures_core::Stream;
use tracing::{info, warn};
use trogon_std::EmptySecret;
use twilight_gateway::{Message, Shard, ShardId};

use crate::runtime_projection::{RuntimeCredentialError, RuntimeCredentialResolver, RuntimeIntegrationKey};
use crate::secret_store::{CredentialKind, SecretStoreError, SecretStoreGet, SourceKind};

use super::config::{DiscordBotToken, DiscordConfig};
use super::gateway::GatewayBridge;

pub async fn run<P: trogon_nats::jetstream::JetStreamPublisher, S: trogon_nats::jetstream::ObjectStorePut>(
    publisher: trogon_nats::jetstream::ClaimCheckPublisher<P, S>,
    config: &DiscordConfig,
) {
    let Some(bot_token) = config.bot_token.clone() else {
        warn!("Discord gateway configured without a static bot token");
        return;
    };
    run_with_bot_token(publisher, config, bot_token).await;
}

pub async fn run_with_runtime_credentials<
    P: trogon_nats::jetstream::JetStreamPublisher,
    S: trogon_nats::jetstream::ObjectStorePut,
    G,
>(
    publisher: trogon_nats::jetstream::ClaimCheckPublisher<P, S>,
    config: &DiscordConfig,
    credentials: RuntimeCredentialResolver<G>,
) where
    G: SecretStoreGet<Error = SecretStoreError>,
{
    match resolve_runtime_bot_token(&credentials).await {
        Ok(bot_token) => run_with_bot_token(publisher, config, bot_token).await,
        Err(error) => warn!(?error, "Discord runtime bot token could not be resolved"),
    }
}

async fn run_with_bot_token<
    P: trogon_nats::jetstream::JetStreamPublisher,
    S: trogon_nats::jetstream::ObjectStorePut,
>(
    publisher: trogon_nats::jetstream::ClaimCheckPublisher<P, S>,
    config: &DiscordConfig,
    bot_token: DiscordBotToken,
) {
    info!("mode: gateway");

    let bridge = GatewayBridge::new(publisher, config.subject_prefix.clone(), config.nats_ack_timeout.into());

    let mut shard = Shard::new(ShardId::ONE, bot_token.as_str().to_owned(), config.intents);

    info!("starting Discord gateway connection");

    loop {
        let msg = poll_fn(|cx| Stream::poll_next(Pin::new(&mut shard), cx)).await;
        match msg {
            Some(Ok(Message::Text(text))) => bridge.dispatch(&text).await,
            Some(Ok(Message::Close(_))) => {
                info!("gateway connection closed");
                break;
            }
            Some(Err(source)) => {
                warn!(?source, "error receiving gateway message");
                continue;
            }
            None => break,
        }
    }
}

pub(crate) async fn resolve_runtime_bot_token<G>(
    credentials: &RuntimeCredentialResolver<G>,
) -> Result<DiscordBotToken, DiscordRuntimeCredentialError>
where
    G: SecretStoreGet<Error = SecretStoreError>,
{
    let token = credentials
        .resolve_plaintext(
            &RuntimeIntegrationKey::for_source(SourceKind::Discord),
            CredentialKind::BotToken,
        )
        .await?;
    DiscordBotToken::new(token.as_str()).map_err(DiscordRuntimeCredentialError::InvalidBotToken)
}

#[derive(Debug, thiserror::Error)]
pub enum DiscordRuntimeCredentialError {
    #[error("discord runtime bot token is invalid")]
    InvalidBotToken(#[source] EmptySecret),
    #[error("discord runtime bot token resolution failed: {0}")]
    Resolve(#[from] RuntimeCredentialError),
}

#[cfg(test)]
mod tests {
    use trogon_decider_runtime::StreamPosition;
    use trogon_std::SecretString;

    use super::*;
    use crate::runtime_projection::RuntimeCredentialRegistry;
    use crate::secret_store::{
        CredentialScope, MockOpenBaoSecretStore, SecretStoreMetadata, SecretStorePut, credential_lifecycle,
        credential_lifecycle::CredentialLifecycleEvent,
    };

    fn owner_id() -> crate::secret_store::CredentialOwnerId {
        crate::secret_store::CredentialOwnerId::new("tenant-1").unwrap()
    }

    fn position(value: u64) -> StreamPosition {
        StreamPosition::try_new(value).unwrap()
    }

    #[tokio::test]
    async fn runtime_bot_token_resolves_from_source_scoped_projection() {
        let store = MockOpenBaoSecretStore::default();
        let credential = store
            .put(
                CredentialScope::source(owner_id(), SourceKind::Discord),
                CredentialKind::BotToken,
                SecretString::new("Bot runtime-token").unwrap(),
            )
            .await
            .unwrap();
        let metadata = store.metadata(&credential).await.unwrap();
        let state = [
            CredentialLifecycleEvent::WriteRequested {
                credential_id: credential.id().clone(),
                owner_id: owner_id(),
                source: SourceKind::Discord,
                kind: CredentialKind::BotToken,
            },
            CredentialLifecycleEvent::Activated { metadata },
        ]
        .into_iter()
        .try_fold(credential_lifecycle::initial_state(), |state, event| {
            credential_lifecycle::evolve(state, &event)
        })
        .unwrap();
        let registry = RuntimeCredentialRegistry::default();
        registry.apply_lifecycle_state(&state, position(2)).await.unwrap();
        let resolver = registry.resolver(store);

        let token = resolve_runtime_bot_token(&resolver).await.unwrap();

        assert_eq!(token.as_str(), "Bot runtime-token");
    }

    #[tokio::test]
    async fn runtime_bot_token_fails_closed_when_projection_is_missing() {
        let registry = RuntimeCredentialRegistry::default();
        let resolver = registry.resolver(MockOpenBaoSecretStore::default());

        let error = resolve_runtime_bot_token(&resolver).await.unwrap_err();

        assert!(matches!(
            error,
            DiscordRuntimeCredentialError::Resolve(RuntimeCredentialError::IntegrationNotFound { .. })
        ));
    }
}
