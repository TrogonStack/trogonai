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
mod tests {
    use super::*;
    use trogon_nats::jetstream::mocks::{MockJetStreamConsumer, MockJetStreamConsumerFactory};

    fn test_prefix() -> A2aPrefix {
        A2aPrefix::new("a2a".to_string()).unwrap()
    }

    fn test_task_id() -> A2aTaskId {
        A2aTaskId::new("task-resub-1").unwrap()
    }

    #[tokio::test]
    async fn open_resubscribe_stream_succeeds_with_valid_consumer() {
        let js = MockJetStreamConsumerFactory::new();
        let (consumer, _tx) = MockJetStreamConsumer::new();
        js.add_consumer(consumer);

        let stream = open_resubscribe_stream(&js, &test_prefix(), &test_task_id(), 42).await;
        assert!(stream.is_ok());
    }

    #[tokio::test]
    async fn returned_stream_has_last_seq_initialized_to_provided_value() {
        let js = MockJetStreamConsumerFactory::new();
        let (consumer, _tx) = MockJetStreamConsumer::new();
        js.add_consumer(consumer);

        let stream = open_resubscribe_stream(&js, &test_prefix(), &test_task_id(), 99).await.unwrap();
        assert_eq!(stream.last_seq(), 99);
    }

    #[tokio::test]
    async fn get_stream_failure_returns_consumer_setup_error() {
        let js = MockJetStreamConsumerFactory::new();
        js.fail_get_stream_at(1);

        let result = open_resubscribe_stream(&js, &test_prefix(), &test_task_id(), 0).await;
        assert!(matches!(result, Err(ClientError::ConsumerSetup(_))));
    }

    #[tokio::test]
    async fn no_available_consumer_returns_consumer_setup_error() {
        let js = MockJetStreamConsumerFactory::new();

        let result = open_resubscribe_stream(&js, &test_prefix(), &test_task_id(), 0).await;
        assert!(matches!(result, Err(ClientError::ConsumerSetup(_))));
    }

    #[tokio::test]
    async fn last_seq_zero_succeeds() {
        let js = MockJetStreamConsumerFactory::new();
        let (consumer, _tx) = MockJetStreamConsumer::new();
        js.add_consumer(consumer);

        let stream = open_resubscribe_stream(&js, &test_prefix(), &test_task_id(), 0).await;
        assert!(stream.is_ok());
        assert_eq!(stream.unwrap().last_seq(), 0);
    }
}
