use serde::{Deserialize, Serialize};

/// Names a tool the sandbox's workload should run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolSelector(pub String);
