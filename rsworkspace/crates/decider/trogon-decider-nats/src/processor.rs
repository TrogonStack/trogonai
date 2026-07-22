//! Durable pull-consumer message processing.
//!
//! [`Processor`] owns the parts of a durable-consumer worker that stay
//! identical across handlers: provisioning the durable consumer via
//! [`jetstream::stream::Stream::create_consumer_strict`] (create it if
//! absent, fail loudly if an existing consumer's configuration diverges,
//! rather than the looser `get_or_create_consumer` used elsewhere in this
//! workspace, which neither closes the create-then-fetch race nor validates
//! configuration), running the pull loop, and turning a [`MessageHandler`]'s
//! [`HandlerVerdict`] into an ack, a delayed nak, or a term. Redelivery is
//! bounded by [`RedeliveryPolicy`]; shutdown is cooperative via a
//! [`CancellationToken`].
//!
//! `create_consumer_strict` requires the `server_2_10` `async-nats` feature,
//! which this crate enables.
//!
//! v1 dispatches every message to one handler; there is no per-key lane
//! dispatch (compare the scheduler's dispatcher, which is schedule-key
//! specific across five generic parameters and does not extract cleanly).
//! Callers needing per-key ordering or dispatch should build it on top of
//! [`Processor`], as the scheduler does today.
//!
//! Consumer-stream-level failures (opening the message stream, reading the
//! next message, and settling ack/nak/term) are treated as fatal for
//! [`Processor::run`] and end the loop; only per-message handler outcomes go
//! through the bounded redelivery path. This crate has no logging
//! dependency, so callers that want visibility into a fatal stop should
//! inspect the returned [`ProcessorError`]; [`MessageHandler::on_poison`] is
//! the equivalent hook for per-message terminal outcomes.

use std::panic::AssertUnwindSafe;
use std::time::Duration;

use async_nats::jetstream;
use async_nats::jetstream::consumer::pull;
use async_nats::jetstream::consumer::{AckPolicy, PullConsumer};
use async_nats::jetstream::message::AckKind;
use async_nats::jetstream::stream::{ConsumerCreateStrictError, ConsumerCreateStrictErrorKind};
use futures::{FutureExt, StreamExt};
use tokio_util::sync::CancellationToken;

/// Verdict a [`MessageHandler`] returns for one message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerVerdict {
    /// The message was handled successfully; acknowledge it.
    Ack,
    /// The message could not be handled right now; redeliver it, subject to
    /// [`RedeliveryPolicy`].
    Retry,
    /// The message can never be handled; stop redelivery without
    /// acknowledging it as processed.
    Poison,
}

/// Why a message stopped being redelivered.
#[derive(Debug)]
pub enum PoisonReason<HandlerError> {
    /// The handler returned [`HandlerVerdict::Poison`].
    Verdict,
    /// The message exhausted [`RedeliveryPolicy::max_deliveries`] attempts.
    RedeliveryExhausted,
    /// The handler returned an error on its final attempt.
    HandlerError(HandlerError),
    /// The handler panicked on its final attempt.
    Panic,
}

/// Handles one durable-consumer message at a time.
pub trait MessageHandler: Send {
    /// Error returned by [`MessageHandler::handle`].
    type Error: std::error::Error + Send + Sync + 'static;

    /// Handles one message, returning the verdict that drives its
    /// acknowledgement.
    fn handle(
        &mut self,
        message: &jetstream::Message,
    ) -> impl std::future::Future<Output = Result<HandlerVerdict, Self::Error>> + Send;

    /// Called when a message is terminated (stops being redelivered)
    /// instead of acknowledged.
    ///
    /// This is the observability hook for terminal outcomes; the default
    /// implementation does nothing.
    fn on_poison(&mut self, reason: PoisonReason<Self::Error>) -> impl std::future::Future<Output = ()> + Send {
        let _ = reason;
        std::future::ready(())
    }
}

/// Bounds how many times a message is redelivered before it is poisoned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RedeliveryPolicy {
    max_deliveries: u32,
    base_delay: Duration,
    max_delay: Duration,
}

impl RedeliveryPolicy {
    /// Creates a redelivery policy with exponential backoff bounded by `max_delay`.
    pub const fn new(max_deliveries: u32, base_delay: Duration, max_delay: Duration) -> Self {
        Self {
            max_deliveries,
            base_delay,
            max_delay,
        }
    }

    /// Returns the maximum number of deliveries before a message is poisoned.
    pub const fn max_deliveries(&self) -> u32 {
        self.max_deliveries
    }

    /// `already_delivered` counts deliveries prior to the one currently
    /// being settled (0 on the first delivery's failure), not the raw
    /// JetStream delivery count, which is 1-indexed.
    fn decide(self, already_delivered: u32) -> RedeliveryDecision {
        if already_delivered.saturating_add(1) >= self.max_deliveries {
            return RedeliveryDecision::Poison;
        }
        let exponent = already_delivered.min(31);
        let multiplier = 1u32.checked_shl(exponent).unwrap_or(u32::MAX);
        let delay = self.base_delay.saturating_mul(multiplier).min(self.max_delay);
        RedeliveryDecision::Retry { delay }
    }
}

impl Default for RedeliveryPolicy {
    fn default() -> Self {
        Self::new(5, Duration::from_millis(200), Duration::from_secs(30))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RedeliveryDecision {
    Retry { delay: Duration },
    Poison,
}

enum Outcome<HandlerError> {
    Ack,
    Retry { delay: Duration },
    Term(PoisonReason<HandlerError>),
}

/// Error raised by [`Processor::run`].
#[derive(Debug, thiserror::Error)]
pub enum ProcessorError {
    /// The durable consumer's ack policy is not [`AckPolicy::Explicit`], so
    /// ack/nak/term settlement would be meaningless.
    #[error("durable consumer '{durable_name}' must use AckPolicy::Explicit")]
    InvalidAckPolicy {
        /// Name of the durable consumer.
        durable_name: String,
    },
    /// Provisioning the durable consumer failed for a reason other than an
    /// existing consumer with a different configuration.
    #[error("failed to provision durable consumer '{durable_name}'")]
    ProvisionConsumer {
        /// Name of the durable consumer.
        durable_name: String,
        /// Underlying `async_nats` error.
        #[source]
        source: ConsumerCreateStrictError,
    },
    /// The durable consumer already existed with a configuration that
    /// diverges from the one requested.
    #[error("durable consumer '{durable_name}' already exists with a different configuration")]
    ConsumerConfigMismatch {
        /// Name of the durable consumer.
        durable_name: String,
    },
    /// Opening the message stream for the durable consumer failed.
    #[error("failed to open message stream for durable consumer '{durable_name}'")]
    OpenMessages {
        /// Name of the durable consumer.
        durable_name: String,
        /// Underlying `async_nats` error.
        #[source]
        source: async_nats::Error,
    },
    /// Reading the next message from the durable consumer failed.
    #[error("failed to read next message from durable consumer '{durable_name}'")]
    ReadMessage {
        /// Name of the durable consumer.
        durable_name: String,
        /// Underlying `async_nats` error.
        #[source]
        source: async_nats::Error,
    },
    /// Reading a message's delivery metadata failed.
    #[error("failed to read delivery metadata for a message")]
    ReadMessageInfo {
        /// Underlying `async_nats` error.
        #[source]
        source: async_nats::Error,
    },
    /// Acknowledging, nak'ing, or terminating a message failed.
    #[error("failed to settle a message")]
    Settle {
        /// Underlying `async_nats` error.
        #[source]
        source: async_nats::Error,
    },
}

async fn provision_consumer(
    stream: &jetstream::stream::Stream,
    config: pull::Config,
) -> Result<PullConsumer, ProcessorError> {
    let durable_name = config.durable_name.clone().unwrap_or_default();
    if config.ack_policy != AckPolicy::Explicit {
        return Err(ProcessorError::InvalidAckPolicy { durable_name });
    }
    match stream.create_consumer_strict(config).await {
        Ok(consumer) => Ok(consumer),
        Err(source) if matches!(source.kind(), ConsumerCreateStrictErrorKind::AlreadyExists) => {
            Err(ProcessorError::ConsumerConfigMismatch { durable_name })
        }
        Err(source) => Err(ProcessorError::ProvisionConsumer { durable_name, source }),
    }
}

/// Drives a durable pull consumer's message loop for one [`MessageHandler`].
pub struct Processor<Handler> {
    stream: jetstream::stream::Stream,
    consumer_config: pull::Config,
    redelivery_policy: RedeliveryPolicy,
    handler: Handler,
}

impl<Handler> Processor<Handler>
where
    Handler: MessageHandler,
{
    /// Creates a processor over `stream` with a durable consumer described
    /// by `consumer_config`.
    ///
    /// `consumer_config.ack_policy` must be [`AckPolicy::Explicit`]; any
    /// other policy is rejected by [`Processor::run`] before the consumer is
    /// provisioned.
    pub fn new(stream: jetstream::stream::Stream, consumer_config: pull::Config, handler: Handler) -> Self {
        Self {
            stream,
            consumer_config,
            redelivery_policy: RedeliveryPolicy::default(),
            handler,
        }
    }

    /// Overrides the default [`RedeliveryPolicy`].
    #[must_use]
    pub fn with_redelivery_policy(mut self, redelivery_policy: RedeliveryPolicy) -> Self {
        self.redelivery_policy = redelivery_policy;
        self
    }

    /// Provisions the durable consumer and runs the pull loop until
    /// `shutdown` is cancelled, the message stream ends, or a fatal error
    /// occurs.
    pub async fn run(mut self, shutdown: CancellationToken) -> Result<(), ProcessorError> {
        let durable_name = self.consumer_config.durable_name.clone().unwrap_or_default();
        let consumer = provision_consumer(&self.stream, self.consumer_config.clone()).await?;
        let mut messages = consumer
            .messages()
            .await
            .map_err(|source| ProcessorError::OpenMessages {
                durable_name: durable_name.clone(),
                source: source.into(),
            })?;

        loop {
            tokio::select! {
                () = shutdown.cancelled() => return Ok(()),
                next = messages.next() => {
                    let Some(message) = next else {
                        return Ok(());
                    };
                    let message = message.map_err(|source| ProcessorError::ReadMessage {
                        durable_name: durable_name.clone(),
                        source: source.into(),
                    })?;
                    self.settle(&message).await?;
                }
            }
        }
    }

    async fn settle(&mut self, message: &jetstream::Message) -> Result<(), ProcessorError> {
        let info = message
            .info()
            .map_err(|source| ProcessorError::ReadMessageInfo { source })?;
        let already_delivered = u32::try_from(info.delivered.max(1))
            .unwrap_or(u32::MAX)
            .saturating_sub(1);

        let outcome = match AssertUnwindSafe(self.handler.handle(message)).catch_unwind().await {
            Ok(Ok(HandlerVerdict::Ack)) => Outcome::Ack,
            Ok(Ok(HandlerVerdict::Poison)) => Outcome::Term(PoisonReason::Verdict),
            Ok(Ok(HandlerVerdict::Retry)) => match self.redelivery_policy.decide(already_delivered) {
                RedeliveryDecision::Retry { delay } => Outcome::Retry { delay },
                RedeliveryDecision::Poison => Outcome::Term(PoisonReason::RedeliveryExhausted),
            },
            Ok(Err(error)) => match self.redelivery_policy.decide(already_delivered) {
                RedeliveryDecision::Retry { delay } => Outcome::Retry { delay },
                RedeliveryDecision::Poison => Outcome::Term(PoisonReason::HandlerError(error)),
            },
            Err(_panic) => match self.redelivery_policy.decide(already_delivered) {
                RedeliveryDecision::Retry { delay } => Outcome::Retry { delay },
                RedeliveryDecision::Poison => Outcome::Term(PoisonReason::Panic),
            },
        };

        match outcome {
            Outcome::Ack => message
                .ack()
                .await
                .map_err(|source| ProcessorError::Settle { source })?,
            Outcome::Retry { delay } => message
                .ack_with(AckKind::Nak(Some(delay)))
                .await
                .map_err(|source| ProcessorError::Settle { source })?,
            Outcome::Term(reason) => {
                message
                    .ack_with(AckKind::Term)
                    .await
                    .map_err(|source| ProcessorError::Settle { source })?;
                self.handler.on_poison(reason).await;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
