use std::sync::{Arc, Mutex};

use trogon_nats::jetstream::{JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageOf, JsMessageRef};

use crate::a2a_prefix::A2aPrefix;
use crate::jetstream::consumers::resubscribe_consumer;
use crate::jetstream::streams::events_stream_name;
use crate::task_id::A2aTaskId;

use super::error::ClientError;
use super::event_stream::{TypedEventStream, build_event_stream};

pub async fn open_resubscribe_stream<J>(
    js: &J,
    prefix: &A2aPrefix,
    task_id: &A2aTaskId,
    last_seq: u64,
) -> Result<TypedEventStream, ClientError>
where
    J: JetStreamGetStream,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
{
    let stream_name = events_stream_name(prefix);
    let stream = js
        .get_stream(&stream_name)
        .await
        .map_err(|e| ClientError::ConsumerSetup(format!("get stream '{stream_name}': {e}")))?;

    let consumer_config = resubscribe_consumer(prefix, task_id, last_seq);

    let consumer = stream
        .create_consumer(consumer_config)
        .await
        .map_err(|e| ClientError::ConsumerSetup(format!("create resubscribe consumer: {e}")))?;

    let last_seq_cell = Arc::new(Mutex::new(last_seq));
    Ok(build_event_stream(consumer, last_seq_cell))
}

#[cfg(test)]
mod tests;
