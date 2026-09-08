use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

use a2a::event::StreamResponse;
use futures::channel::mpsc;
use futures::{SinkExt, Stream, StreamExt};
use tokio::task::AbortHandle;
use trogon_nats::jetstream::{JetStreamConsumer, JsAck, JsMessageRef};

use crate::constants::EVENT_STREAM_QUEUE_CAPACITY;
use crate::req_id::ReqId;

use super::error::ClientError;
use super::wire::{decode_response, map_wire_error};

pub struct TypedEventStream {
    receiver: mpsc::Receiver<Result<StreamResponse, ClientError>>,
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

/// Pump a task-events consumer into a typed stream.
///
/// `demux_req_id` is `Some` for a live subscription, where the consumer's subject
/// names only the task (ADR#0055) and concurrent subscribers to that task are told
/// apart by `Trogon-Req-Id`. It is `None` for a resume, whose replayed events carry
/// the correlation id of the original subscription rather than of this request.
pub fn build_event_stream<C>(
    consumer: C,
    last_seq_cell: Arc<Mutex<u64>>,
    demux_req_id: Option<ReqId>,
) -> TypedEventStream
where
    C: JetStreamConsumer + Send + 'static,
    C::Message: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    C::Messages: Send + 'static,
    C::MessagesError: std::fmt::Display + Send + 'static,
    C::StreamError: std::fmt::Display + Send + 'static,
{
    let (tx, receiver) = mpsc::channel(EVENT_STREAM_QUEUE_CAPACITY);
    let last_seq_for_task = last_seq_cell.clone();

    let join = tokio::spawn(pull_loop(consumer, tx, last_seq_for_task, demux_req_id));

    TypedEventStream {
        receiver,
        last_seq: last_seq_cell,
        abort: join.abort_handle(),
    }
}

/// A stream that is already finished.
///
/// `message/stream` may answer with a bare `Message` instead of a `Task`. There is
/// no task to scope a consumer to and no events will follow, so the caller gets a
/// stream that ends immediately rather than a consumer on a subject nobody writes.
pub fn empty_event_stream() -> TypedEventStream {
    let (tx, receiver) = mpsc::channel::<Result<StreamResponse, ClientError>>(EVENT_STREAM_QUEUE_CAPACITY);
    drop(tx);
    let join = tokio::spawn(std::future::ready(()));

    TypedEventStream {
        receiver,
        last_seq: Arc::new(Mutex::new(0)),
        abort: join.abort_handle(),
    }
}

async fn pull_loop<C>(
    consumer: C,
    mut tx: mpsc::Sender<Result<StreamResponse, ClientError>>,
    last_seq: Arc<Mutex<u64>>,
    demux_req_id: Option<ReqId>,
) where
    C: JetStreamConsumer,
    C::Message: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    C::MessagesError: std::fmt::Display,
    C::StreamError: std::fmt::Display,
{
    let mut msgs = match consumer.messages().await {
        Ok(m) => m,
        Err(e) => {
            let _ = tx.send(Err(ClientError::ConsumerSetup(e.to_string()))).await;
            return;
        }
    };

    while let Some(item) = msgs.next().await {
        match item {
            Err(e) => {
                let _ = tx.send(Err(ClientError::JetStream(e.to_string()))).await;
                return;
            }
            Ok(js_msg) => {
                // One task can be streamed by several callers at once, so an event
                // stamped for someone else's subscription is acked and dropped
                // rather than handed to this stream's reader.
                if let Some(ref req_id) = demux_req_id
                    && !req_id.matches_event_headers(js_msg.message().headers.as_ref())
                {
                    if let Err(e) = js_msg.ack().await {
                        tracing::warn!(error = %e, "JetStream ack failed for an event of another subscription");
                    }
                    continue;
                }

                let stream_seq = stream_sequence_from_reply(js_msg.message().reply.as_deref());
                let message = js_msg.message();
                let event_headers = message.headers.clone().unwrap_or_default();
                let send_result = tx.send(decode_event(&event_headers, message.payload.as_ref())).await;

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

/// Each task event on the wire is a full JSON-RPC success response repeating the
/// request id, which is the shape the A2A spec puts on `message/stream` and
/// `tasks/resubscribe`. The result member is the `StreamResponse` the reader wants.
///
/// A body that is not an envelope gets one more chance as a
/// [`crate::task_event`] an older agent published, because refusing those would
/// end a stream mid-task on every replay and every rolling upgrade.
fn decode_event(headers: &async_nats::header::HeaderMap, payload: &[u8]) -> Result<StreamResponse, ClientError> {
    match decode_response::<StreamResponse>(headers, payload) {
        Ok(Ok(event)) => Ok(event),
        Ok(Err((code, message))) => Err(ClientError::from_jsonrpc_code(code, message)),
        Err(e) => crate::task_event::decode_legacy_event(payload).ok_or_else(|| map_wire_error(e)),
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
