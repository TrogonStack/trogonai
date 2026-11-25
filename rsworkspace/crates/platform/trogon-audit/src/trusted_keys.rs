use std::collections::HashSet;

use crate::key_id::KeyId;

/// The key ids a verifier accepts checkpoints from. A key id names its public
/// key, so trusting a key is holding its id, and a rotated-out key stays here
/// for as long as its checkpoints are still checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustedKeys(HashSet<KeyId>);

impl TrustedKeys {
    pub fn new(keys: impl IntoIterator<Item = KeyId>) -> Self {
        Self(keys.into_iter().collect())
    }

    pub fn contains(&self, key: &KeyId) -> bool {
        self.0.contains(key)
    }
}

#[cfg(test)]
mod tests;
