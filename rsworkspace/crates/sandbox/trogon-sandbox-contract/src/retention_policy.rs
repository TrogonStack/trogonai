use serde::{Deserialize, Serialize};

use crate::{EvictedArtifactAction, RetentionPolicyError};

/// How many snapshot tags a policy keeps and for how long.
///
/// Deserialization does not validate so stored policies keep parsing; [`RetentionPolicy::new`] and
/// [`RetentionPolicy::validate`] enforce the `max_tags` bounds.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct RetentionPolicy {
    pub max_tags: u32,
    pub max_age_days: u32,
    #[serde(default)]
    pub delete_evicted: EvictedArtifactAction,
}

impl RetentionPolicy {
    pub const MIN_MAX_TAGS: u32 = 1;
    pub const MAX_MAX_TAGS: u32 = 10;

    pub fn new(
        max_tags: u32,
        max_age_days: u32,
        delete_evicted: EvictedArtifactAction,
    ) -> Result<Self, RetentionPolicyError> {
        let policy = Self {
            max_tags,
            max_age_days,
            delete_evicted,
        };
        policy.validate()?;
        Ok(policy)
    }

    pub fn validate(&self) -> Result<(), RetentionPolicyError> {
        if !(Self::MIN_MAX_TAGS..=Self::MAX_MAX_TAGS).contains(&self.max_tags) {
            return Err(RetentionPolicyError::MaxTagsOutOfRange {
                min: Self::MIN_MAX_TAGS,
                max: Self::MAX_MAX_TAGS,
            });
        }
        Ok(())
    }
}
