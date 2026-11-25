use serde::{Deserialize, Serialize};

/// What to do with a snapshot that records no environment digest because it predates the field.
///
/// A rollout starts with `allow_with_warning` while old snapshots age out, then flips to `refuse`.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacySnapshotPolicy {
    #[default]
    AllowWithWarning,
    Refuse,
}
