use serde::{Deserialize, Serialize};

/// Every model purpose must have an explicit route, including incidental calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelPurpose {
    Worker,
    Reviewer,
    Title,
    Summary,
    Compaction,
    Embedding,
    Search,
}
