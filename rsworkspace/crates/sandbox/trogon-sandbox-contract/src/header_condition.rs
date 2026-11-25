use serde::{Deserialize, Serialize};

use crate::{HeaderName, HeaderValue};

/// A header the request must carry: by name alone, or with exactly `value` on at least one occurrence.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct HeaderCondition {
    pub name: HeaderName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<HeaderValue>,
}
