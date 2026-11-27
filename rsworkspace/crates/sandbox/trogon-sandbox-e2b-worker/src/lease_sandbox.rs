use crate::envd::EnvdTarget;

/// What a per-lease verb needs to reach the sandbox a lease holds.
#[derive(Debug, Clone)]
pub struct LeaseSandbox {
    pub sandbox_id: String,
    pub target: EnvdTarget,
    pub traffic_token: Option<String>,
    pub domain: String,
    pub exposed_ports: Vec<u16>,
}
