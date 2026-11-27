use trogon_sandbox_contract::Port;

use crate::naming::{ActorName, Atespace};

/// A lease this worker currently holds.
#[derive(Debug, Clone)]
pub struct ActiveLease {
    pub atespace: Atespace,
    pub actor: ActorName,
    /// Kept so `cancel` knows whether the actor may outlive the lease.
    pub named: bool,
    pub persistent: bool,
    pub exposed_port: Option<Port>,
}

impl ActiveLease {
    pub fn sandbox_id(&self) -> String {
        format!("{}/{}", self.atespace.as_str(), self.actor.as_str())
    }
}
