//! Crate-wide constants.

use std::num::NonZeroU64;

use trogon_decider::SnapshotCadence;

pub(crate) const RESERVED_SCHEDULE_HEADERS: [&str; 5] = [
    "Nats-Schedule",
    "Nats-Schedule-Source",
    "Nats-Schedule-Target",
    "Nats-Schedule-Time-Zone",
    "Nats-Schedule-TTL",
];

const COMMAND_SNAPSHOT_EVERY: NonZeroU64 = NonZeroU64::new(32).unwrap();

/// Cadence every state-bearing scheduler command declares.
///
/// `CreateSchedule` is deliberately absent: it only ever writes the first event of a stream, so a
/// snapshot taken right after it would save one event of replay and cost a KV write.
pub const COMMAND_SNAPSHOT_CADENCE: SnapshotCadence = SnapshotCadence::EveryEvents(COMMAND_SNAPSHOT_EVERY);
