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

#[allow(dead_code)]
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

#[allow(dead_code)]
fn map_last_raw_message_error(error: LastRawMessageError) -> VerificationTokenError {
    if error.kind() == LastRawMessageErrorKind::NoMessageFound {
        VerificationTokenError::NoVerificationRequest
    } else {
        VerificationTokenError::LastMessage(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_nats::client::SubscribeErrorKind;
    use async_nats::jetstream::context::GetStreamErrorKind;
    use async_nats::jetstream::message::StreamMessage;
    use async_nats::jetstream::stream::LastRawMessageError;
    use bytes::Bytes;
    use time::OffsetDateTime;
    use trogon_nats::MockNatsClient;
    use trogon_nats::jetstream::{MockJetStreamConsumerFactory, StreamMaxAge};
    use trogon_nats::mocks::MockError;
    use trogon_std::NonZeroDuration;

    fn config() -> NotionConfig {
        NotionConfig {
            verification_token: NotionVerificationToken::new("configured-token").unwrap(),
            subject_prefix: NatsToken::new("notion-primary").unwrap(),
            stream_name: NatsToken::new("NOTION_PRIMARY").unwrap(),
            stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
            nats_ack_timeout: NonZeroDuration::from_secs(10).unwrap(),
        }
    }

    #[test]
    fn verification_subject_uses_configured_prefix() {
        assert_eq!(
            verification_subject(&config().subject_prefix),
            "notion-primary.subscription.verification"
        );
    }

    #[test]
    fn parse_token_reads_verification_token() {
        let token = parse_token(br#"{"verification_token":"secret_token"}"#).unwrap();

        assert_eq!(token.as_str(), "secret_token");
    }

    #[test]
    fn parse_token_rejects_missing_token() {
        let error = parse_token(br#"{}"#).unwrap_err();

        assert!(matches!(error, VerificationTokenError::MissingVerificationToken));
    }

    #[test]
    fn parse_token_rejects_empty_token() {
        let error = parse_token(br#"{"verification_token":""}"#).unwrap_err();

        assert!(matches!(error, VerificationTokenError::InvalidVerificationToken(_)));
    }

    #[tokio::test]
    async fn latest_reads_last_raw_message_from_configured_stream_and_subject() {
        let js = MockJetStreamConsumerFactory::new();
        js.add_last_raw_message(stream_message(
            "notion-primary.subscription.verification",
            Bytes::from_static(br#"{"verification_token":"secret_token"}"#),
        ));

        let token = latest(&js, &config()).await.unwrap();

        assert_eq!(token.as_str(), "secret_token");
        assert_eq!(js.get_stream_calls(), vec!["NOTION_PRIMARY"]);
        assert_eq!(
            js.last_raw_message_subjects(),
            vec!["notion-primary.subscription.verification"]
        );
    }

    #[tokio::test]
    async fn latest_maps_no_raw_message_to_retry_error() {
        let js = MockJetStreamConsumerFactory::new();

        let error = latest(&js, &config()).await.unwrap_err();

        assert!(matches!(error, VerificationTokenError::NoVerificationRequest));
    }

    #[tokio::test]
    async fn latest_preserves_get_stream_error() {
        let js = MockJetStreamConsumerFactory::new();
        js.fail_get_stream_at(1);

        let error = latest(&js, &config()).await.unwrap_err();

        assert!(matches!(error, VerificationTokenError::Stream(_)));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[tokio::test]
    async fn latest_preserves_last_message_error() {
        let js = MockJetStreamConsumerFactory::new();
        js.add_last_raw_message_error(LastRawMessageError::new(LastRawMessageErrorKind::Other));

        let error = latest(&js, &config()).await.unwrap_err();

        assert!(matches!(error, VerificationTokenError::LastMessage(_)));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[test]
    fn error_display_messages_are_operator_facing() {
        let invalid_json = serde_json::from_slice::<serde_json::Value>(b"{").unwrap_err();
        let invalid_token = NotionVerificationToken::new("").unwrap_err();

        let cases = [
            (
                VerificationTokenError::NoVerificationRequest,
                "no verification request received yet; trigger 'Verify endpoint' in Notion and retry",
            ),
            (
                VerificationTokenError::InvalidVerificationRequest(invalid_json),
                "verification request payload is not valid JSON",
            ),
            (
                VerificationTokenError::MissingVerificationToken,
                "verification request payload is missing verification_token",
            ),
            (
                VerificationTokenError::InvalidVerificationToken(invalid_token),
                "verification_token must not be empty",
            ),
            (
                VerificationTokenError::Stream(GetStreamError::with_source(
                    GetStreamErrorKind::Request,
                    MockError("mock get stream error".into()),
                )),
                "failed to open Notion JetStream stream",
            ),
            (
                VerificationTokenError::LastMessage(LastRawMessageError::new(LastRawMessageErrorKind::Other)),
                "failed to read latest Notion verification request",
            ),
            (
                VerificationTokenError::Subscribe(NatsSubscribeError::with_source(
                    SubscribeErrorKind::Other,
                    MockError("mock subscribe error".into()),
                )),
                "failed to watch Notion verification requests",
            ),
            (
                VerificationTokenError::WatchEnded,
                "verification request watch ended before receiving a token",
            ),
        ];

        for (error, message) in cases {
            assert_eq!(error.to_string(), message);
        }
    }

    #[test]
    fn error_source_tracks_wrapped_failures() {
        let invalid_json = serde_json::from_slice::<serde_json::Value>(b"{").unwrap_err();
        let invalid_token = NotionVerificationToken::new("").unwrap_err();

        let errors_with_source = [
            VerificationTokenError::InvalidVerificationRequest(invalid_json),
            VerificationTokenError::InvalidVerificationToken(invalid_token),
            VerificationTokenError::Stream(GetStreamError::with_source(
                GetStreamErrorKind::Request,
                MockError("mock get stream error".into()),
            )),
            VerificationTokenError::LastMessage(LastRawMessageError::new(LastRawMessageErrorKind::Other)),
            VerificationTokenError::Subscribe(NatsSubscribeError::with_source(
                SubscribeErrorKind::Other,
                MockError("mock subscribe error".into()),
            )),
        ];

        for error in errors_with_source {
            assert!(std::error::Error::source(&error).is_some());
        }

        let errors_without_source = [
            VerificationTokenError::NoVerificationRequest,
            VerificationTokenError::MissingVerificationToken,
            VerificationTokenError::WatchEnded,
        ];

        for error in errors_without_source {
            assert!(std::error::Error::source(&error).is_none());
        }
    }

    #[tokio::test]
    async fn watch_reads_first_subscription_message_from_configured_subject() {
        let nats = MockNatsClient::new();
        let messages = nats.inject_messages();
        messages
            .unbounded_send(nats_message(
                "notion-primary.subscription.verification",
                br#"{"verification_token":"secret_token"}"#,
            ))
            .unwrap();
        drop(messages);

        let token = watch(&nats, &config()).await.unwrap();

        assert_eq!(token.as_str(), "secret_token");
        assert_eq!(nats.subscribed_to(), vec!["notion-primary.subscription.verification"]);
    }

    #[tokio::test]
    async fn watch_preserves_subscribe_error() {
        let nats = MockNatsClient::new();

        let error = watch(&nats, &config()).await.unwrap_err();

        assert!(matches!(error, VerificationTokenError::Subscribe(_)));
        assert!(std::error::Error::source(&error).is_some());
    }

    #[tokio::test]
    async fn watch_maps_closed_subscription_to_watch_ended() {
        let nats = MockNatsClient::new();
        drop(nats.inject_messages());

        let error = watch(&nats, &config()).await.unwrap_err();

        assert!(matches!(error, VerificationTokenError::WatchEnded));
    }

    fn nats_message(subject: &str, payload: &'static [u8]) -> async_nats::Message {
        async_nats::Message {
            subject: subject.into(),
            reply: None,
            payload: Bytes::from_static(payload),
            headers: None,
            status: None,
            description: None,
            length: payload.len(),
        }
    }

    fn stream_message(subject: &str, payload: Bytes) -> StreamMessage {
        StreamMessage {
            subject: subject.into(),
            sequence: 1,
            headers: async_nats::HeaderMap::new(),
            payload,
            time: OffsetDateTime::UNIX_EPOCH,
        }
    }
}
