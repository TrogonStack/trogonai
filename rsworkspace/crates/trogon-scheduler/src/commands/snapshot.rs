use std::num::NonZeroU64;

use trogon_decider_runtime::FrequencySnapshot;

const COMMAND_SNAPSHOT_EVERY: NonZeroU64 = NonZeroU64::new(32).unwrap();

pub(crate) const COMMAND_SNAPSHOT_POLICY: FrequencySnapshot = FrequencySnapshot::new(COMMAND_SNAPSHOT_EVERY);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_snapshot_policy_uses_expected_frequency() {
        assert_eq!(COMMAND_SNAPSHOT_POLICY.frequency().get(), 32);
        assert_eq!(COMMAND_SNAPSHOT_POLICY.frequency(), COMMAND_SNAPSHOT_EVERY);
    }
}
