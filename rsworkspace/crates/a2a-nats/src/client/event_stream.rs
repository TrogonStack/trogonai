use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

use a2a::event::StreamResponse;
use futures::channel::mpsc;
use futures::{Stream, StreamExt};
use trogon_nats::jetstream::{JetStreamConsumer, JsAck, JsMessageRef};

use super::error::ClientError;

pub struct TypedEventStream {
    receiver: mpsc::UnboundedReceiver<Result<StreamResponse, ClientError>>,
    last_seq: Arc<Mutex<u64>>,
}

impl TypedEventStream {
    pub fn last_seq(&self) -> u64 {
        *through_poison(self.last_seq.lock())
    }
}

impl Stream for TypedEventStream {
    type Item = Result<StreamResponse, ClientError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.receiver).poll_next(cx)
    }
}

pub fn build_event_stream<C>(consumer: C, last_seq_cell: Arc<Mutex<u64>>) -> TypedEventStream
where
    C: JetStreamConsumer + Send + 'static,
    C::Message: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    C::Messages: Send + 'static,
    C::MessagesError: std::fmt::Display + Send + 'static,
    C::StreamError: std::fmt::Display + Send + 'static,
{
    let (tx, receiver) = mpsc::unbounded();
    let last_seq_for_task = last_seq_cell.clone();

    tokio::spawn(async move {
        let mut msgs = match consumer.messages().await {
            Ok(m) => m,
            Err(e) => {
                let _ = tx.unbounded_send(Err(ClientError::ConsumerSetup(e.to_string())));
                return;
            }
        };

        while let Some(item) = msgs.next().await {
            match item {
                Err(e) => {
                    let _ = tx.unbounded_send(Err(ClientError::JetStream(e.to_string())));
                    return;
                }
                Ok(js_msg) => {
                    let payload = js_msg.message().payload.as_ref();
                    let parsed: Result<StreamResponse, _> = serde_json::from_slice(payload);

                    if let Some(seq) = extract_sequence(js_msg.message()) {
                        let mut guard = through_poison(last_seq_for_task.lock());
                        if seq > *guard {
                            *guard = seq;
                        }
                    }

                    let _ = js_msg.ack().await;

                    match parsed {
                        Err(e) => {
                            let _ = tx.unbounded_send(Err(ClientError::Deserialize(e)));
                        }
                        Ok(event) => {
                            let _ = tx.unbounded_send(Ok(event));
                        }
                    }
                }
            }
        }
    });

    TypedEventStream {
        receiver,
        last_seq: last_seq_cell,
    }
}

fn extract_sequence(msg: &async_nats::Message) -> Option<u64> {
    msg.headers
        .as_ref()
        .and_then(|h| h.get("Nats-Sequence"))
        .and_then(|v| v.as_str().parse().ok())
}

/// Recover from a poisoned mutex by reading through the poison — the cell only
/// holds a `u64` sequence counter, so a partial write from a panicking thread
/// can't leave it logically corrupt. Re-panicking every reader after one
/// unrelated panic would just propagate the failure.
fn through_poison<'a, T>(result: Result<MutexGuard<'a, T>, PoisonError<MutexGuard<'a, T>>>) -> MutexGuard<'a, T> {
    match result {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

#[cfg(test)]
mod tests {
    use a2a::event::TaskStatusUpdateEvent;
    use a2a::types::{TaskState, TaskStatus};
    use bytes::Bytes;
    use trogon_nats::jetstream::mocks::{MockJetStreamConsumer, MockJsMessage};

    use super::*;

    fn make_status_event(task_id: &str) -> StreamResponse {
        StreamResponse::StatusUpdate(TaskStatusUpdateEvent {
            task_id: task_id.to_string(),
            context_id: "ctx".to_string(),
            status: TaskStatus {
                state: TaskState::Working,
                message: None,
                timestamp: None,
            },
            metadata: None,
        })
    }

    fn nats_msg_with_seq(payload: Vec<u8>, seq: Option<u64>) -> async_nats::Message {
        let headers = seq.map(|s| {
            let mut h = async_nats::HeaderMap::new();
            h.insert("Nats-Sequence", s.to_string().as_str());
            h
        });
        async_nats::Message {
            subject: "a2a.tasks.t1.events.r1".into(),
            reply: None,
            payload: Bytes::from(payload),
            headers,
            status: None,
            description: None,
            length: 0,
        }
    }

    #[tokio::test]
    async fn stream_yields_deserialized_events() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let mut stream = build_event_stream(consumer, last_seq.clone());

        let event = make_status_event("task-1");
        let payload = serde_json::to_vec(&event).unwrap();
        tx.unbounded_send(Ok(MockJsMessage::new(nats_msg_with_seq(payload, None))))
            .unwrap();
        drop(tx);

        let item = stream.next().await;
        assert!(item.is_some());
        assert!(item.unwrap().is_ok());
    }

    #[tokio::test]
    async fn stream_closes_when_sender_dropped() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let mut stream = build_event_stream(consumer, last_seq);
        drop(tx);

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn stream_yields_error_on_bad_payload() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let mut stream = build_event_stream(consumer, last_seq);

        tx.unbounded_send(Ok(MockJsMessage::new(nats_msg_with_seq(b"not json".to_vec(), None))))
            .unwrap();
        drop(tx);

        let item = stream.next().await;
        assert!(matches!(item, Some(Err(ClientError::Deserialize(_)))));
    }

    #[tokio::test]
    async fn stream_yields_error_on_consumer_stream_error() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let mut stream = build_event_stream(consumer, last_seq);

        tx.unbounded_send(Err(trogon_nats::mocks::MockError("boom".to_string())))
            .unwrap();
        drop(tx);

        let item = stream.next().await;
        assert!(matches!(item, Some(Err(ClientError::JetStream(_)))));
    }

    #[tokio::test]
    async fn consumer_setup_failure_emits_consumer_setup_error() {
        let consumer = MockJetStreamConsumer::failing();
        let last_seq = Arc::new(Mutex::new(0u64));
        let mut stream = build_event_stream(consumer, last_seq);

        let item = stream.next().await;
        assert!(matches!(item, Some(Err(ClientError::ConsumerSetup(_)))));
    }

    #[tokio::test]
    async fn last_seq_advances_on_messages_with_sequence_header() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let mut stream = build_event_stream(consumer, last_seq.clone());

        let event = make_status_event("task-1");
        let payload = serde_json::to_vec(&event).unwrap();
        tx.unbounded_send(Ok(MockJsMessage::new(nats_msg_with_seq(payload, Some(7)))))
            .unwrap();
        drop(tx);

        let _ = stream.next().await;
        let _ = stream.next().await;
        assert_eq!(stream.last_seq(), 7);
    }

    #[tokio::test]
    async fn last_seq_starts_at_zero() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let stream = build_event_stream(consumer, last_seq.clone());
        drop(tx);
        drop(stream);
        assert_eq!(*through_poison(last_seq.lock()), 0);
    }

    #[tokio::test]
    async fn last_seq_is_accessible_on_stream() {
        let (consumer, tx) = MockJetStreamConsumer::new();
        let last_seq = Arc::new(Mutex::new(0u64));
        let stream = build_event_stream(consumer, last_seq.clone());
        drop(tx);
        assert_eq!(stream.last_seq(), 0);
    }

    #[tokio::test]
    async fn extract_sequence_with_header() {
        let mut headers = async_nats::HeaderMap::new();
        headers.insert("Nats-Sequence", "42");
        let msg = async_nats::Message {
            subject: "test".into(),
            reply: None,
            payload: bytes::Bytes::new(),
            headers: Some(headers),
            status: None,
            description: None,
            length: 0,
        };
        assert_eq!(extract_sequence(&msg), Some(42));
    }

    #[test]
    fn extract_sequence_without_header_returns_none() {
        let msg = async_nats::Message {
            subject: "test".into(),
            reply: None,
            payload: bytes::Bytes::new(),
            headers: None,
            status: None,
            description: None,
            length: 0,
        };
        assert_eq!(extract_sequence(&msg), None);
    }

    #[test]
    fn through_poison_recovers_from_poisoned_lock() {
        let m = Arc::new(Mutex::new(0u64));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("intentional");
        })
        .join();
        assert!(m.lock().is_err());
        let mut g = through_poison(m.lock());
        *g = 5;
        drop(g);
        assert_eq!(*through_poison(m.lock()), 5);
    }
}
