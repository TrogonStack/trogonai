use std::fmt::Debug;

use futures_util::{Stream, StreamExt};
use tracing::{info, warn};
use trogon_std::EmptySecretError;
use twilight_gateway::Message;

use crate::credential::commands::domain::{CredentialKind, SourceKind};
use crate::credential_runtime_projection::{RuntimeCredentialError, RuntimeCredentialResolver, RuntimeIntegrationKey};
use crate::secret_store::{SecretStoreError, SecretStoreGet};
use crate::supervision::RestartBackoff;

use super::config::DiscordBotToken;
use super::gateway::GatewayBridge;

pub async fn run<P, S, Incoming, E>(
    publisher: trogon_nats::jetstream::ClaimCheckPublisher<P, S>,
    config: super::config::DiscordConfig,
    mut incoming: Incoming,
) where
    P: trogon_nats::jetstream::JetStreamPublisher,
    S: trogon_nats::jetstream::ObjectStorePut,
    Incoming: Stream<Item = Result<Message, E>> + Unpin,
    E: Debug,
{
    info!("mode: gateway");

    let bridge = GatewayBridge::new(publisher, config.subject_prefix.clone(), config.nats_ack_timeout.into());

    info!("starting Discord gateway connection");

    loop {
        let msg = incoming.next().await;
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

pub(crate) async fn resolve_runtime_bot_token_with_retry<G>(
    credentials: &RuntimeCredentialResolver<G>,
    backoff: RestartBackoff,
) -> DiscordBotToken
where
    G: SecretStoreGet<Error = SecretStoreError>,
{
    let mut attempt: u32 = 1;
    let mut delay = backoff.initial();
    loop {
        match resolve_runtime_bot_token(credentials).await {
            Ok(bot_token) => return bot_token,
            Err(error) => {
                warn!(?error, attempt, retry_in = ?delay, "Discord runtime bot token could not be resolved");
            }
        }
        tokio::time::sleep(delay).await;
        attempt += 1;
        delay = backoff.next(delay);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DiscordRuntimeCredentialError {
    #[error("discord runtime bot token is invalid")]
    InvalidBotToken(#[source] EmptySecretError),
    #[error("discord runtime bot token resolution failed: {0}")]
    Resolve(#[from] RuntimeCredentialError),
}

#[cfg(test)]
mod runtime_credential_tests {
    use std::time::Duration;

    use trogon_decider_runtime::StreamPosition;
    use trogon_std::SecretString;
    use trogon_std::log_capture::{CapturedEvents, LevelFilter};

    use super::*;
    use crate::credential::commands::domain::{CredentialOwnerId, CredentialScope};
    use crate::credential::commands::proto::{activated_to_proto, write_requested_to_proto};
    use crate::credential::{evolve, initial_state};
    use crate::credential_runtime_projection::{RuntimeCredentialRegistry, RuntimeIntegrationProjection};
    use crate::secret_store::{MockOpenBaoSecretStore, SecretStoreMetadata, SecretStorePut};
    use trogonai_proto::gateway::credentials::v1;

    fn owner_id() -> CredentialOwnerId {
        CredentialOwnerId::new("tenant-1").unwrap()
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
            v1::CredentialEvent {
                event: Some(
                    write_requested_to_proto(
                        credential.id(),
                        &CredentialScope::source(owner_id(), SourceKind::Discord),
                        CredentialKind::BotToken,
                    )
                    .into(),
                ),
            },
            v1::CredentialEvent {
                event: Some(activated_to_proto(&metadata).into()),
            },
        ]
        .into_iter()
        .try_fold(initial_state(), |state, event| evolve(state, &event))
        .unwrap();
        let registry = RuntimeCredentialRegistry::default();
        registry.apply_state(&state, position(2)).await.unwrap();
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

    #[tokio::test(start_paused = true)]
    async fn runtime_bot_token_retry_backs_off_until_the_projection_catches_up() {
        let events = CapturedEvents::new();
        let _guard = events.install(LevelFilter::INFO);
        let store = MockOpenBaoSecretStore::default();
        let registry = RuntimeCredentialRegistry::default();
        let resolver = registry.resolver(store.clone());
        let retry = resolve_runtime_bot_token_with_retry(
            &resolver,
            RestartBackoff::new(Duration::from_secs(1), Duration::from_secs(30)),
        );
        tokio::pin!(retry);

        assert!(
            tokio::time::timeout(Duration::from_millis(2500), &mut retry)
                .await
                .is_err()
        );
        let credential = store
            .put(
                CredentialScope::source(owner_id(), SourceKind::Discord),
                CredentialKind::BotToken,
                SecretString::new("Bot runtime-token").unwrap(),
            )
            .await
            .unwrap();
        registry
            .projections()
            .upsert(RuntimeIntegrationProjection::active_from_credential_ref(credential, 1).unwrap())
            .await;
        let token = tokio::time::timeout(Duration::from_secs(5), retry).await.unwrap();

        assert_eq!(token.as_str(), "Bot runtime-token");
        let captured = events.events();
        let failures: Vec<_> = captured
            .iter()
            .filter(|event| event.message() == Some("Discord runtime bot token could not be resolved"))
            .map(|event| (event.field("attempt"), event.field("retry_in")))
            .collect();
        assert_eq!(failures, [(Some("1"), Some("1s")), (Some("2"), Some("2s"))]);
        assert!(
            captured
                .iter()
                .all(|event| !format!("{event:?}").contains("runtime-token"))
        );
    }
}

#[cfg(test)]
mod tests;
