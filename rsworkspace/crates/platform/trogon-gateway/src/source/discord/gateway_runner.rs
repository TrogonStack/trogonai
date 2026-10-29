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
mod runtime_credential_tests;

#[cfg(test)]
mod tests;
