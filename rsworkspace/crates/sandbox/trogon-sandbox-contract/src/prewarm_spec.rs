use serde::{Deserialize, Serialize};

use crate::ProviderKind;

/// A pool of warm sandboxes to keep ready from a snapshot, and the providers that may host them.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct PrewarmSpec {
    pub pool_size: u32,
    pub providers: Vec<ProviderKind>,
}
