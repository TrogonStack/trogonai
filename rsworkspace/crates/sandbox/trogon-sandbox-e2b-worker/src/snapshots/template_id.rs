use serde::{Deserialize, Serialize};

use super::constants::MAX_TEMPLATE_ID_BYTES;
use super::template_id_error::SnapshotTemplateIdError;

/// The vendor template identity of a captured snapshot: lowercase letters, digits, `-` and `_`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct SnapshotTemplateId(String);

impl TryFrom<String> for SnapshotTemplateId {
    type Error = SnapshotTemplateIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let canonical = !value.is_empty()
            && value.len() <= MAX_TEMPLATE_ID_BYTES
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_'));
        if !canonical {
            return Err(SnapshotTemplateIdError::Invalid);
        }
        Ok(Self(value))
    }
}

impl From<SnapshotTemplateId> for String {
    fn from(value: SnapshotTemplateId) -> Self {
        value.0
    }
}

impl SnapshotTemplateId {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}
