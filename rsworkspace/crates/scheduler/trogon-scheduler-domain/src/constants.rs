//! Crate-wide constants.

#[cfg(feature = "runtime-snapshot")]
use std::num::NonZeroU64;

#[cfg(feature = "runtime-snapshot")]
use trogon_decider_runtime::FrequencySnapshot;

pub(crate) const MAX_LENGTH: usize = 256;

pub(crate) const RESERVED_SCHEDULE_HEADERS: [&str; 5] = [
    "Nats-Schedule",
    "Nats-Schedule-Source",
    "Nats-Schedule-Target",
    "Nats-Schedule-Time-Zone",
    "Nats-Schedule-TTL",
];

#[cfg(feature = "runtime-snapshot")]
const COMMAND_SNAPSHOT_EVERY: NonZeroU64 = NonZeroU64::new(32).unwrap();

#[cfg(feature = "runtime-snapshot")]
pub const COMMAND_SNAPSHOT_POLICY: FrequencySnapshot = FrequencySnapshot::new(COMMAND_SNAPSHOT_EVERY);
