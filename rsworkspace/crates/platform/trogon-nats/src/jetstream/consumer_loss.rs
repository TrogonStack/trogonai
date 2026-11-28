//! Reading "pull again from a new consumer" out of a pull-stream error.
//!
//! A pull stream keeps yielding errors once the consumer it pulls from is gone
//! (deleted, or lost when NATS restarted without its storage) and never
//! recovers on its own. Missed heartbeats during a partition are different:
//! the stream resumes once the connection is back.

use async_nats::jetstream::consumer::pull::{MessagesError, MessagesErrorKind};

/// Errors on a consumer's message stream that only a new subscription clears.
pub trait ConsumerLoss {
    /// True when the caller must recreate the consumer and pull again.
    ///
    /// A deleted consumer answers `ConsumerDeleted` or, once NATS forgot it,
    /// `NoResponders`. A deleted stream shows up as a failed pull request, so
    /// `Pull` counts too; recreating a consumer that still exists is harmless.
    fn consumer_lost(&self) -> bool;
}

impl ConsumerLoss for MessagesError {
    fn consumer_lost(&self) -> bool {
        matches!(
            self.kind(),
            MessagesErrorKind::ConsumerDeleted | MessagesErrorKind::NoResponders | MessagesErrorKind::Pull
        )
    }
}

#[cfg(test)]
mod tests;
