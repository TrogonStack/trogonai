use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A live lease cap applied to each value of one tag key, with the current count per value.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct TagLimits {
    pub tag_key: String,
    /// Live leases allowed for each value of `tag_key`.
    pub max_active_leases: u32,
    /// Live leases now, per value of `tag_key`. Values with none are omitted.
    #[serde(default)]
    pub in_use: BTreeMap<String, u32>,
}
