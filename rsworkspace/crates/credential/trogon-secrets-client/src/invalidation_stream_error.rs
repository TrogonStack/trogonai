use async_nats::jetstream::stream::{Config, RetentionPolicy};

use super::constants::INVALIDATION_LOOKBACK;

/// Why a client refuses the invalidation stream: a setting that could remove
/// or rewrite a notice before [`INVALIDATION_LOOKBACK`] has passed, so a
/// reader starting that far back could miss it with no gap to show.
///
/// The reader checks the consumer sequences it is delivered, not the stream
/// sequences behind them. A notice removed before it is delivered leaves the
/// consumer sequences unbroken, so the stream itself must not remove one
/// inside the lookback; within a stream this accepts, none leaves it there.
///
/// Capacity is the operator's, as the secrets service leaves it: the discard
/// policy, `max_bytes` and `max_messages` are not checked. A notice lost to
/// such a bound is bounded by the hold of the copy it named (ADR#0049), not
/// by the reader.
#[derive(Debug, thiserror::Error, Eq, PartialEq)]
pub enum InvalidationStreamError {
    #[error("the invalidation stream keeps notices for less than the lookback")]
    MaxAge,
    #[error("the invalidation stream lets a notice expire on its own TTL")]
    MessageTtl,
    #[error("the invalidation stream keeps only the latest notices of each subject")]
    MaxMessagesPerSubject,
    #[error("the invalidation stream removes notices once they are consumed")]
    Retention,
    #[error("the invalidation stream lets a notice be deleted")]
    DeletesAllowed,
    #[error("the invalidation stream lets its notices be purged")]
    PurgesAllowed,
    #[error("the invalidation stream lets a rollup replace its notices")]
    Rollup,
    #[error("the invalidation stream rewrites the subjects it stores")]
    SubjectTransform,
}

impl InvalidationStreamError {
    /// Checks that `config` removes and rewrites no notice inside the
    /// lookback, naming the first setting that could.
    pub(crate) fn check(config: &Config) -> Result<(), Self> {
        if !config.max_age.is_zero() && config.max_age < INVALIDATION_LOOKBACK {
            return Err(Self::MaxAge);
        }
        if config.allow_message_ttl {
            return Err(Self::MessageTtl);
        }
        if config.max_messages_per_subject > 0 {
            return Err(Self::MaxMessagesPerSubject);
        }
        if config.retention != RetentionPolicy::Limits {
            return Err(Self::Retention);
        }
        if !config.deny_delete {
            return Err(Self::DeletesAllowed);
        }
        if !config.deny_purge {
            return Err(Self::PurgesAllowed);
        }
        if config.allow_rollup {
            return Err(Self::Rollup);
        }
        if config.subject_transform.is_some() {
            return Err(Self::SubjectTransform);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
