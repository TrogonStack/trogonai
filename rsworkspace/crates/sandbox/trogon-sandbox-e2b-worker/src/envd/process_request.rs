use std::collections::BTreeMap;
use std::time::Duration;

/// One process to run inside the sandbox.
#[derive(Debug, Clone)]
pub struct ProcessRequest {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<String>,
    pub user: String,
    pub timeout: Duration,
}
