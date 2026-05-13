use std::fmt;

use async_nats::client::SubscribeError as NatsSubscribeError;
use async_nats::jetstream::context::GetStreamError;
use async_nats::jetstream::stream::{LastRawMessageError, LastRawMessageErrorKind};
use futures_util::StreamExt;
use trogon_nats::jetstream::{JetStreamGetStream, JetStreamLastRawMessageBySubject};
use trogon_nats::{NatsToken, SubscribeClient};

use super::NotionVerificationToken;
use super::config::NotionConfig;

#[derive(Debug)]
pub enum VerificationTokenError {
    NoVerificationRequest,
    InvalidVerificationRequest(serde_json::Error),
    MissingVerificationToken,
    InvalidVerificationToken(trogon_std::EmptySecret),
    Stream(GetStreamError),
    LastMessage(LastRawMessageError),
    Subscribe(NatsSubscribeError),
    WatchEnded,
}

impl fmt::Display for VerificationTokenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoVerificationRequest => {
                f.write_str("no verification request received yet; trigger 'Verify endpoint' in Notion and retry")
            }
            Self::InvalidVerificationRequest(_) => f.write_str("verification request payload is not valid JSON"),
            Self::MissingVerificationToken => f.write_str("verification request payload is missing verification_token"),
            Self::InvalidVerificationToken(_) => f.write_str("verification_token must not be empty"),
            Self::Stream(_) => f.write_str("failed to open Notion JetStream stream"),
            Self::LastMessage(_) => f.write_str("failed to read latest Notion verification request"),
            Self::Subscribe(_) => f.write_str("failed to watch Notion verification requests"),
            Self::WatchEnded => f.write_str("verification request watch ended before receiving a token"),
        }
    }
}

impl std::error::Error for VerificationTokenError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidVerificationRequest(error) => Some(error),
            Self::InvalidVerificationToken(error) => Some(error),
            Self::Stream(error) => Some(error),
            Self::LastMessage(error) => Some(error),
            Self::Subscribe(error) => Some(error),
            Self::NoVerificationRequest | Self::MissingVerificationToken | Self::WatchEnded => None,
        }
    }
}

pub(crate) async fn latest<J>(js: &J, config: &NotionConfig) -> Result<NotionVerificationToken, VerificationTokenError>
where
    J: JetStreamGetStream<Error = GetStreamError>,
    J::Stream: JetStreamLastRawMessageBySubject,
{
    let stream = js
        .get_stream(config.stream_name.as_str())
        .await
        .map_err(VerificationTokenError::Stream)?;
    let subject = verification_subject(&config.subject_prefix);
    let message = stream
        .get_last_raw_message_by_subject(&subject)
        .await
        .map_err(map_last_raw_message_error)?;

    parse_token(&message.payload)
}

pub async fn watch<N>(nats: &N, config: &NotionConfig) -> Result<NotionVerificationToken, VerificationTokenError>
where
    N: SubscribeClient<SubscribeError = NatsSubscribeError>,
{
    let subject = verification_subject(&config.subject_prefix);
    let mut subscriber = nats
        .subscribe(subject)
        .await
        .map_err(VerificationTokenError::Subscribe)?;

    if let Some(message) = subscriber.next().await {
        parse_token(&message.payload)
    } else {
        Err(VerificationTokenError::WatchEnded)
    }
}

pub fn verification_subject(subject_prefix: &NatsToken) -> String {
    format!("{subject_prefix}.subscription.verification")
}

fn parse_token(body: &[u8]) -> Result<NotionVerificationToken, VerificationTokenError> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(VerificationTokenError::InvalidVerificationRequest)?;
    let token = value
        .get("verification_token")
        .and_then(serde_json::Value::as_str)
        .ok_or(VerificationTokenError::MissingVerificationToken)?;

    NotionVerificationToken::new(token).map_err(VerificationTokenError::InvalidVerificationToken)
}

fn map_last_raw_message_error(error: LastRawMessageError) -> VerificationTokenError {
    if error.kind() == LastRawMessageErrorKind::NoMessageFound {
        VerificationTokenError::NoVerificationRequest
    } else {
        VerificationTokenError::LastMessage(error)
    }
}

#[cfg(test)]
mod tests;
