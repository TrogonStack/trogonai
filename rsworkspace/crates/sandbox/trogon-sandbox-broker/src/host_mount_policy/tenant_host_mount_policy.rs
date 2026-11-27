use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantHostMountPolicy {
    pub path_prefixes: Vec<String>,
}
