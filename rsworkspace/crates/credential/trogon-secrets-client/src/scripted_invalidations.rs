//! A stand-in for the invalidation stream that the crate's unit tests feed
//! by hand.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, PoisonError};

use async_nats::jetstream::consumer::pull::{self, MessagesError, MessagesErrorKind};
use async_nats::jetstream::consumer::{StreamError, StreamErrorKind};
use async_nats::jetstream::context::{ConsumerInfoError, ConsumerInfoErrorKind, GetStreamError, GetStreamErrorKind};
use async_nats::jetstream::stream::{self, ConsumerError, ConsumerErrorKind};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

use super::fixtures::invalidation_stream_config;
use super::{InvalidationConsumer, InvalidationStep, InvalidationStream, InvalidationStreamSource, ScriptedNotice};

type Delivery = Result<ScriptedNotice, MessagesError>;
type Feed = UnboundedSender<Delivery>;

/// Answers every lookup with itself and hands each reader a feed of its own;
/// the test sends notices down the latest feed, numbered from one as a
/// consumer numbers what it delivers. The stream is configured as the
/// secrets service creates it until a test configures it otherwise.
#[derive(Clone)]
pub(crate) struct ScriptedInvalidations {
    stream_config: Arc<Mutex<stream::Config>>,
    refusals: Arc<Mutex<VecDeque<InvalidationStep>>>,
    streams: Arc<Mutex<Vec<String>>>,
    configs: Arc<Mutex<Vec<pull::Config>>>,
    feed: Arc<Mutex<Option<Feed>>>,
    delivered: Arc<Mutex<u64>>,
    backlog: Arc<Mutex<u64>>,
    held_back: Arc<Mutex<Vec<ScriptedNotice>>>,
    position_checks: Arc<Mutex<usize>>,
}

impl Default for ScriptedInvalidations {
    fn default() -> Self {
        Self {
            stream_config: Arc::new(Mutex::new(invalidation_stream_config())),
            refusals: Arc::default(),
            streams: Arc::default(),
            configs: Arc::default(),
            feed: Arc::default(),
            delivered: Arc::default(),
            backlog: Arc::default(),
            held_back: Arc::default(),
            position_checks: Arc::default(),
        }
    }
}

impl ScriptedInvalidations {
    /// Configures the stream every later lookup finds as `config`.
    pub(crate) fn configure_stream(&self, config: stream::Config) {
        *lock(&self.stream_config) = config;
    }
    /// Makes the next attempt fail at `step`.
    pub(crate) fn refuse(&self, step: InvalidationStep) {
        lock(&self.refusals).push_back(step);
    }

    /// Makes the stream hold `count` notices for the next consumer created,
    /// which the test then delivers with [`ScriptedInvalidations::notify`].
    pub(crate) fn replay(&self, count: u64) {
        *lock(&self.backlog) = count;
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

    /// Counts a notice published on `subject` as delivered but keeps it from
    /// the reader until [`ScriptedInvalidations::deliver_held_back`], as a
    /// notice still on its way, or lost on it, is.
    pub(crate) fn hold_back(&self, subject: &str) {
        let sequence = self.number();
        lock(&self.held_back).push(ScriptedNotice::new(subject, Some(sequence)));
    }

    /// Delivers every notice held back, in the order they were numbered.
    pub(crate) fn deliver_held_back(&self) {
        let held_back = std::mem::take(&mut *lock(&self.held_back));
        for notice in held_back {
            self.send(Ok(notice));
        }
    }

    /// How many times a reader asked how far its consumer delivered.
    pub(crate) fn position_checks(&self) -> usize {
        *lock(&self.position_checks)
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

    fn config(&self) -> stream::Config {
        lock(&self.stream_config).clone()
    }

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
        *lock(&self.backlog) = 0;
        Ok(notices)
    }

    fn backlog(&self) -> u64 {
        *lock(&self.backlog)
    }

    async fn delivered(&self) -> Result<u64, ConsumerInfoError> {
        *lock(&self.position_checks) += 1;
        if self.refuses(InvalidationStep::Position) {
            return Err(ConsumerInfoError::new(ConsumerInfoErrorKind::TimedOut));
        }
        Ok(*lock(&self.delivered))
    }
}

fn lock<T>(shared: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    shared.lock().unwrap_or_else(PoisonError::into_inner)
}
