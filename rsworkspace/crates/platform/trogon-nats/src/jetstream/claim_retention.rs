use std::time::Duration;

use trogon_std::NonZeroDuration;

use super::stream_max_age::StreamMaxAge;

/// Retention for the object-store bucket that backs claim-check payloads.
///
/// A claim object is the out-of-line body of a message that carries a claim
/// reference instead of an inline payload. It must stay resolvable for as long
/// as the message that references it can still be delivered, so its retention is
/// not a free choice: it is dictated by the stream the claim publisher writes
/// to. Constructing this through [`ClaimRetention::tracking`] keeps the bucket
/// TTL coupled to the stream and makes the two durability contracts explicit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClaimRetention {
    /// Claim objects are transport for a bounded stream. The bucket expires
    /// objects after the stream's retention plus a `grace` window, so a message
    /// still on the stream can always resolve its claim.
    TracksStream {
        stream_max_age: NonZeroDuration,
        grace: Duration,
    },
    /// Claim objects are the payload of an event-sourced stream. They never
    /// expire: the bytes live as long as the log references them and are
    /// reclaimed only through an explicit erasure event, never by a bucket TTL.
    EventSourced,
}

impl ClaimRetention {
    /// Derive the retention that tracks `stream_max_age`. A bounded stream maps
    /// to [`ClaimRetention::TracksStream`]; a stream that never expires maps to
    /// [`ClaimRetention::EventSourced`], so the bucket never expires either and
    /// a claim can always be resolved.
    pub fn tracking(stream_max_age: StreamMaxAge, grace: Duration) -> Self {
        match stream_max_age {
            StreamMaxAge::NoExpiry => Self::EventSourced,
            StreamMaxAge::ExpireAfter(stream_max_age) => Self::TracksStream { stream_max_age, grace },
        }
    }

    /// The `max_age` to apply to the object-store bucket. `Duration::ZERO` is
    /// how NATS spells "no expiry", which is the [`ClaimRetention::EventSourced`]
    /// case.
    pub fn bucket_max_age(&self) -> Duration {
        match self {
            Self::TracksStream { stream_max_age, grace } => Duration::from(*stream_max_age) + *grace,
            Self::EventSourced => Duration::ZERO,
        }
    }
}

#[cfg(test)]
mod tests;
