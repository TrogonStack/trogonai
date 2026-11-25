//! The delivery acknowledgement queue (`deliveryUploader`, bridge.mjs offset 1042101) behind
//! `POST /worker/events/delivery`: the `received` and `processed` acknowledgements of `reportDelivery` wait their
//! turn, go out in batches, and are retried with a backoff until the service takes them or refuses them for good.
//!
//! The queue does no I/O. [`DeliveryQueue::poll`] says what to do next and [`DeliveryQueue::done`],
//! [`DeliveryQueue::failed`] or [`DeliveryQueue::dropped`] take the answer, which is the order the upstream `drain`
//! loop runs in. The first acknowledgements queued while the queue is idle leave on their own, because the upstream
//! queue starts draining on the call that enqueues them; whatever is queued while that request is in flight goes out
//! together afterwards.
//!
//! The `processing` acknowledgements never enter this queue: they leave in one request of their own, with no retry
//! (`sendProcessingAcks`, offset 1138528), which the worker client sends.

use std::collections::VecDeque;
use std::time::Duration;

use serde_json::{Map, Value};

use crate::bridge_worker_events::retry_delay;

mod constants;

pub use constants::{MAX_BATCH_ACKS, MAX_QUEUE_ACKS};

/// What a delivery acknowledgement reports about an inbound event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeliveryStatus {
    Received,
    Processing,
    Processed,
}

impl DeliveryStatus {
    /// The status as the request body spells it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Processing => "processing",
            Self::Processed => "processed",
        }
    }
}

/// One acknowledgement of an inbound event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryAck {
    pub event_id: String,
    pub status: DeliveryStatus,
}

/// What [`DeliveryQueue::poll`] asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum DeliveryNext {
    /// Nothing is waiting.
    Idle,
    /// Wait before the next request, the retry delay of the failure that came before.
    Pause(Duration),
    /// Send these acknowledgements in one request.
    Send(Vec<DeliveryAck>),
}

/// The queue behind `POST /worker/events/delivery`.
#[derive(Debug, Default)]
pub struct DeliveryQueue {
    pending: VecDeque<DeliveryAck>,
    in_flight: Vec<DeliveryAck>,
    failures: u32,
    wait: Option<Duration>,
    draining: bool,
    first: Option<usize>,
    closed: bool,
    dropped_batches: u32,
}

impl DeliveryQueue {
    /// Whether `count` more acknowledgements fit (`hasRoomFor`).
    pub fn has_room_for(&self, count: usize) -> bool {
        !self.closed && self.pending.len() + count <= MAX_QUEUE_ACKS
    }

    /// Whether the queue was closed.
    pub fn is_closed(&self) -> bool {
        self.closed
    }

    /// Whether [`DeliveryQueue::poll`] would have nothing to do.
    pub fn is_idle(&self) -> bool {
        self.closed || (self.wait.is_none() && self.pending.is_empty())
    }

    /// Acknowledgements waiting for a request, not counting the batch in flight.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// How many batches the service refused for good (`droppedBatchCount`).
    pub fn dropped_batches(&self) -> u32 {
        self.dropped_batches
    }

    /// Queues acknowledgements (`enqueue`). A closed queue drops them.
    pub fn enqueue(&mut self, acks: Vec<DeliveryAck>) {
        if self.closed || acks.is_empty() {
            return;
        }
        if !self.draining {
            self.draining = true;
            self.first = Some(acks.len().min(MAX_BATCH_ACKS));
        }
        self.pending.extend(acks);
    }

    /// Drops what the queue holds and stops it for good (`close`).
    pub fn close(&mut self) {
        self.closed = true;
        self.pending.clear();
        self.in_flight.clear();
        self.wait = None;
    }

    /// What to do next.
    pub fn poll(&mut self) -> DeliveryNext {
        if self.closed {
            return DeliveryNext::Idle;
        }
        if let Some(delay) = self.wait.take() {
            return DeliveryNext::Pause(delay);
        }
        if self.pending.is_empty() {
            self.draining = false;
            self.first = None;
            return DeliveryNext::Idle;
        }
        let count = self
            .first
            .take()
            .unwrap_or(MAX_BATCH_ACKS)
            .min(MAX_BATCH_ACKS)
            .min(self.pending.len());
        self.in_flight = self.pending.drain(..count).collect();
        DeliveryNext::Send(self.in_flight.clone())
    }

    /// The batch in flight was taken: the next batch goes out at once.
    pub fn done(&mut self) {
        self.failures = 0;
        self.in_flight.clear();
    }

    /// The service refused the batch in flight for good: it is let go and counted.
    pub fn dropped(&mut self) {
        self.dropped_batches += 1;
        self.done();
    }

    /// The batch in flight failed and goes back to the front of the queue, `jitter` being a fresh value in
    /// `[0, 1)` and `retry_after_ms` what a `429` asked for.
    pub fn failed(&mut self, jitter: f64, retry_after_ms: Option<f64>) {
        if self.closed {
            return;
        }
        self.failures += 1;
        for ack in std::mem::take(&mut self.in_flight).into_iter().rev() {
            self.pending.push_front(ack);
        }
        self.wait = Some(retry_delay(self.failures, retry_after_ms, jitter));
    }
}

/// The body of one `POST /worker/events/delivery`.
pub fn delivery_body(epoch: i64, acks: &[DeliveryAck]) -> Value {
    let updates = acks
        .iter()
        .map(|ack| {
            let mut update = Map::new();
            update.insert("event_id".to_owned(), Value::String(ack.event_id.clone()));
            update.insert("status".to_owned(), Value::String(ack.status.as_str().to_owned()));
            Value::Object(update)
        })
        .collect();
    let mut body = Map::new();
    body.insert("worker_epoch".to_owned(), Value::from(epoch));
    body.insert("updates".to_owned(), Value::Array(updates));
    Value::Object(body)
}
