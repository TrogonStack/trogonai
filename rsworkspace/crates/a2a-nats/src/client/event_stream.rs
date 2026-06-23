use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

use a2a::event::StreamResponse;
use futures::channel::mpsc;
use futures::{Stream, StreamExt};
use tokio::task::AbortHandle;
use trogon_nats::jetstream::{JetStreamConsumer, JsAck, JsMessageRef};

use super::error::ClientError;

pub struct TypedEventStream {
    receiver: mpsc::UnboundedReceiver<Result<StreamResponse, ClientError>>,
    last_seq: Arc<Mutex<u64>>,
    abort: AbortHandle,
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

impl Drop for TypedEventStream {
    fn drop(&mut self) {
        // Without this, the spawned pull loop survives the receiver drop and
        // sits awaiting `msgs.next()` until JetStream delivers another message,
        // holding the consumer open as a zombie task.
        self.abort.abort();
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

    let join = tokio::spawn(pull_loop(consumer, tx, last_seq_for_task));

    TypedEventStream {
        receiver,
        last_seq: last_seq_cell,
        abort: join.abort_handle(),
    }
}

async fn pull_loop<C>(
    consumer: C,
    tx: mpsc::UnboundedSender<Result<StreamResponse, ClientError>>,
    last_seq: Arc<Mutex<u64>>,
) where
    C: JetStreamConsumer,
    C::Message: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    C::MessagesError: std::fmt::Display,
    C::StreamError: std::fmt::Display,
{
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
                let stream_seq = stream_sequence_from_reply(js_msg.message().reply.as_deref());
                let payload = js_msg.message().payload.as_ref();
                let send_result = match serde_json::from_slice::<StreamResponse>(payload) {
                    Ok(event) => tx.unbounded_send(Ok(event)),
                    Err(e) => tx.unbounded_send(Err(ClientError::Deserialize(e))),
                };

                if send_result.is_err() {
                    return;
                }

                if let Some(seq) = stream_seq {
                    let mut guard = through_poison(last_seq.lock());
                    if seq > *guard {
                        *guard = seq;
                    }
                }

                if let Err(e) = js_msg.ack().await {
                    tracing::warn!(error = %e, "JetStream ack failed; event already delivered downstream");
                }
            }
        }
    }
}

/// Parse the JetStream stream sequence from the message reply subject.
///
/// JetStream encodes delivery metadata in the reply subject:
/// `$JS.ACK.<stream>.<consumer>.<delivered>.<stream_seq>.<consumer_seq>.<timestamp>.<pending>`.
/// The EVENTS stream does not configure republish, so the `Nats-Sequence` header
/// is absent on these messages — parsing the reply subject is the only reliable
/// source for the cursor that `resubscribe_consumer` later resumes from.
fn stream_sequence_from_reply(reply: Option<&str>) -> Option<u64> {
    let parts: Vec<&str> = reply?.split('.').collect();
    if parts.len() < 9 || parts[0] != "$JS" || parts[1] != "ACK" {
        return None;
    }
    parts[5].parse().ok()
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
mod tests;
