use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::build_id_error::SnapshotBuildIdError;

/// The immutable build of a captured snapshot: a non-nil UUID in its hyphenated lowercase form.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct SnapshotBuildId(Uuid);

impl TryFrom<String> for SnapshotBuildId {
    type Error = SnapshotBuildIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let parsed = Uuid::parse_str(&value).map_err(|_| SnapshotBuildIdError::Malformed)?;
        if parsed.is_nil() || parsed.hyphenated().to_string() != value {
            return Err(SnapshotBuildIdError::NotCanonical);
        }
        Ok(Self(parsed))
    }
}

impl From<SnapshotBuildId> for String {
    fn from(value: SnapshotBuildId) -> Self {
        value.0.hyphenated().to_string()
    }
}
