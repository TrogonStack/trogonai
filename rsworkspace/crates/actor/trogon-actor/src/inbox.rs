use std::time::Duration;

use async_nats::jetstream::stream;
use trogon_nats::jetstream::JetStreamContext;

/// Name of the JetStream stream that holds all actor inbox messages.
pub const ACTOR_INBOX_STREAM: &str = "ACTOR_INBOX";

/// How long NATS will wait for an ACK before redelivering a message.
///
/// Must exceed [`crate::host::DEFAULT_EVENT_TIMEOUT`] (60 s) so NATS doesn't
/// redeliver a message while the actor is still processing it.
pub const ACK_WAIT: Duration = Duration::from_secs(90);

/// Maximum delivery attempts before a message is considered undeliverable.
pub const MAX_DELIVER: i64 = 5;

/// Provision the `ACTOR_INBOX` JetStream stream.
///
/// The stream captures every `actors.>` subject and uses `WorkQueue`
/// retention: each message is deleted as soon as one consumer ACKs it,
/// ensuring exactly-once processing per entity-actor instance.
///
/// This is idempotent — safe to call at every startup in both the router
/// (before dispatching) and each actor host (before consuming).
pub async fn provision_actor_inbox<J: JetStreamContext>(js: &J) -> Result<(), J::Error> {
    js.get_or_create_stream(stream::Config {
        name: ACTOR_INBOX_STREAM.to_string(),
        subjects: vec!["actors.>".to_string()],
        retention: stream::RetentionPolicy::WorkQueue,
        ..Default::default()
    })
    .await?;
    Ok(())
}
