use std::num::NonZeroU64;

use trogon_eventsourcing::FrequencySnapshot;

const COMMAND_SNAPSHOT_EVERY: NonZeroU64 = NonZeroU64::new(32).expect("command snapshot cadence must be non-zero");

pub(crate) const COMMAND_SNAPSHOT_POLICY: FrequencySnapshot = FrequencySnapshot::new(COMMAND_SNAPSHOT_EVERY);
