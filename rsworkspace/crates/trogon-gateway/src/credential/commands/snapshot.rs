use std::num::NonZeroU64;

use trogon_decider_runtime::FrequencySnapshot;

const CREDENTIAL_SNAPSHOT_EVERY: NonZeroU64 = NonZeroU64::new(32).unwrap();

pub(crate) const CREDENTIAL_SNAPSHOT_POLICY: FrequencySnapshot = FrequencySnapshot::new(CREDENTIAL_SNAPSHOT_EVERY);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_snapshot_policy_uses_expected_frequency() {
        assert_eq!(CREDENTIAL_SNAPSHOT_POLICY.frequency().get(), 32);
        assert_eq!(CREDENTIAL_SNAPSHOT_POLICY.frequency(), CREDENTIAL_SNAPSHOT_EVERY);
    }
}
