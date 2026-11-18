use std::fmt;

use async_nats::jetstream::consumer::pull::MessagesError;
use async_nats::jetstream::context::ConsumerInfoError;
use trogon_semconv::attribute::SecretsClientInvalidationLossReason;

/// Why the invalidation reader lost its place in the invalidation stream.
#[derive(Debug)]
pub(crate) enum InvalidationLoss {
    /// The consumer failed while its notices were being read.
    Failed(MessagesError),
    /// A notice arrived out of turn, so one before it was skipped.
    Gap { expected: u64, arrived: u64 },
    /// A notice arrived without saying where it falls in what the consumer
    /// delivered.
    Unplaced(async_nats::Error),
    /// The consumer stopped delivering notices.
    Ended,
    /// Notices the server says it delivered never arrived.
    Missing { received: u64, delivered: u64 },
    /// The server could not say how far the consumer delivered.
    PositionUnknown(ConsumerInfoError),
}

impl InvalidationLoss {
    /// The reason recorded with the loss, one of a fixed few.
    pub(crate) const fn reason(&self) -> SecretsClientInvalidationLossReason {
        match self {
            Self::Failed(_) => SecretsClientInvalidationLossReason::Failed,
            Self::Gap { .. } => SecretsClientInvalidationLossReason::Gap,
            Self::Unplaced(_) => SecretsClientInvalidationLossReason::Unplaced,
            Self::Ended => SecretsClientInvalidationLossReason::Ended,
            Self::Missing { .. } => SecretsClientInvalidationLossReason::Missing,
            Self::PositionUnknown(_) => SecretsClientInvalidationLossReason::PositionUnknown,
        }
    }
}

impl fmt::Display for InvalidationLoss {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Failed(failed) => write!(formatter, "the invalidation consumer failed: {failed}"),
            Self::Gap { expected, arrived } => {
                write!(formatter, "notice {arrived} arrived where notice {expected} was due")
            }
            Self::Unplaced(unplaced) => write!(
                formatter,
                "a notice arrived without its place in the delivery: {unplaced}"
            ),
            Self::Ended => formatter.write_str("the invalidation consumer stopped delivering notices"),
            Self::Missing { received, delivered } => write!(
                formatter,
                "notices through {delivered} were delivered, but only through {received} arrived"
            ),
            Self::PositionUnknown(unknown) => write!(
                formatter,
                "the server could not say how far the invalidation consumer delivered: {unknown}"
            ),
        }
    }
}

#[cfg(test)]
mod tests;
