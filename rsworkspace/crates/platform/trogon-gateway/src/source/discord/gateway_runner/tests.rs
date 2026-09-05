use std::io;

use futures_util::stream;
use trogon_nats::NatsToken;
use trogon_nats::jetstream::{
    ClaimBucket, ClaimBucketBinding, ClaimCheckPublisher, MaxPayload, MockJetStreamPublisher, MockObjectStore,
    StreamMaxAge,
};
use trogon_std::NonZeroDuration;

use super::*;
use crate::source::discord::config::{DiscordBotToken, DiscordConfig};

fn config() -> DiscordConfig {
    DiscordConfig {
        bot_token: DiscordBotToken::new("fixture-token").unwrap(),
        intents: twilight_model::gateway::Intents::GUILDS,
        subject_prefix: NatsToken::new("discord").unwrap(),
        stream_name: NatsToken::new("DISCORD").unwrap(),
        stream_max_age: StreamMaxAge::from_secs(3600).unwrap(),
        nats_ack_timeout: NonZeroDuration::from_secs(5).unwrap(),
    }
}

fn publisher(mock: MockJetStreamPublisher) -> ClaimCheckPublisher<MockJetStreamPublisher, MockObjectStore> {
    ClaimCheckPublisher::new(
        mock,
        ClaimBucketBinding::for_test(MockObjectStore::new(), ClaimBucket::default()),
        MaxPayload::from_server_limit(usize::MAX),
    )
}

#[tokio::test]
async fn receive_errors_do_not_discard_later_events_before_end_of_stream() {
    let mock = MockJetStreamPublisher::new();
    let incoming = stream::iter([
        Err(io::Error::from(io::ErrorKind::ConnectionReset)),
        Ok(Message::Text(
            r#"{"op":0,"t":"MESSAGE_CREATE","d":{"id":"7","guild_id":"42"}}"#.to_owned(),
        )),
    ]);
    run(publisher(mock.clone()), config(), incoming).await;
    assert_eq!(mock.published_subjects(), ["discord.message_create"]);
    let messages = mock.published_messages();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&messages[0].payload).unwrap(),
        serde_json::json!({"id": "7", "guild_id": "42"})
    );
}

#[tokio::test]
async fn close_frame_stops_before_dispatching_subsequent_messages() {
    let mock = MockJetStreamPublisher::new();
    let incoming = stream::iter([
        Ok::<_, io::Error>(Message::Close(None)),
        Ok(Message::Text(r#"{"op":0,"t":"READY","d":{}}"#.to_owned())),
    ]);
    run(publisher(mock.clone()), config(), incoming).await;
    assert!(mock.published_messages().is_empty());
}
