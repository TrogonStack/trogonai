use serde::{Deserialize, Serialize};

use crate::{EvictedArtifactAction, RetentionTagLimit};

/// How many snapshot tags a policy keeps and for how long.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub max_tags: RetentionTagLimit,
    pub max_age_days: u32,
    #[serde(default)]
    pub delete_evicted: EvictedArtifactAction,
}
