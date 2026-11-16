//! A stand-in for the invalidation stream that the crate's unit tests feed
//! by hand.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use async_nats::jetstream::consumer::pull::{self, MessagesError, MessagesErrorKind};
use async_nats::jetstream::consumer::{StreamError, StreamErrorKind};
use async_nats::jetstream::context::{GetStreamError, GetStreamErrorKind};
use async_nats::jetstream::stream::{ConsumerError, ConsumerErrorKind};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use super::{InvalidationConsumer, InvalidationStep, InvalidationStream, InvalidationStreamSource, ScriptedNotice};

type Delivery = Result<ScriptedNotice, MessagesError>;
type Feed = UnboundedSender<Delivery>;

/// Answers every lookup with itself and hands each reader a feed of its own;
/// the test sends notices down the latest feed, numbered from one as a
/// consumer numbers what it delivers.
#[derive(Clone, Default)]
pub(crate) struct ScriptedInvalidations {
    refusals: Arc<Mutex<VecDeque<InvalidationStep>>>,
    streams: Arc<Mutex<Vec<String>>>,
    configs: Arc<Mutex<Vec<pull::Config>>>,
    feed: Arc<Mutex<Option<Feed>>>,
    delivered: Arc<Mutex<u64>>,
}

impl ScriptedInvalidations {
    /// Makes the next attempt fail at `step`.
    pub(crate) fn refuse(&self, step: InvalidationStep) {
        lock(&self.refusals).push_back(step);
    }

    /// Delivers a notice published on `subject` to the reader.
    pub(crate) fn notify(&self, subject: &str) {
        let sequence = self.number();
        self.send(Ok(ScriptedNotice::new(subject, Some(sequence))));
    }

    /// Delivers the notice after the next, as a consumer that lost one does.
    pub(crate) fn skip_one(&self, subject: &str) {
        self.number();
        self.notify(subject);
    }

    /// Delivers a notice whose place in the delivery cannot be read.
    pub(crate) fn notify_unplaced(&self, subject: &str) {
        self.send(Ok(ScriptedNotice::new(subject, None)));
    }

    /// Makes the reader's consumer fail, as one that lost its place does.
    pub(crate) fn fail(&self) {
        self.send(Err(MessagesError::new(MessagesErrorKind::MissingHeartbeat)));
    }

    /// Ends the reader's feed, as a consumer whose stream went away does.
    pub(crate) fn end(&self) {
        lock(&self.feed).take();
    }

    /// Whether a reader still holds the latest feed.
    pub(crate) fn is_read(&self) -> bool {
        lock(&self.feed).as_ref().is_some_and(|feed| !feed.is_closed())
    }

    pub(crate) fn streams(&self) -> Vec<String> {
        lock(&self.streams).clone()
    }

    pub(crate) fn configs(&self) -> Vec<pull::Config> {
        lock(&self.configs).clone()
    }

    fn number(&self) -> u64 {
        let mut delivered = lock(&self.delivered);
        *delivered += 1;
        *delivered
    }

    fn send(&self, notice: Delivery) {
        if let Some(feed) = lock(&self.feed).as_ref() {
            let _ = feed.unbounded_send(notice);
        }
    }

    fn refuses(&self, step: InvalidationStep) -> bool {
        let mut refusals = lock(&self.refusals);
        if refusals.front() == Some(&step) {
            refusals.pop_front();
            return true;
        }
        false
    }
}

impl InvalidationStreamSource for ScriptedInvalidations {
    type Stream = Self;

    async fn get_stream(&self, name: &str) -> Result<Self, GetStreamError> {
        lock(&self.streams).push(name.to_owned());
        if self.refuses(InvalidationStep::Stream) {
            return Err(GetStreamError::new(GetStreamErrorKind::Request));
        }
        Ok(self.clone())
    }
}

impl InvalidationStream for ScriptedInvalidations {
    type Consumer = Self;

    async fn create_consumer(&self, config: pull::Config) -> Result<Self, ConsumerError> {
        lock(&self.configs).push(config);
        if self.refuses(InvalidationStep::Consumer) {
            return Err(ConsumerError::new(ConsumerErrorKind::TimedOut));
        }
        Ok(self.clone())
    }
}

impl InvalidationConsumer for ScriptedInvalidations {
    type Notice = ScriptedNotice;
    type Notices = UnboundedReceiver<Delivery>;

    async fn messages(&self) -> Result<Self::Notices, StreamError> {
        if self.refuses(InvalidationStep::Messages) {
            return Err(StreamError::new(StreamErrorKind::TimedOut));
        }
        let (feed, notices) = unbounded();
        lock(&self.feed).replace(feed);
        *lock(&self.delivered) = 0;
        Ok(notices)
    }
}

fn lock<T>(shared: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}
