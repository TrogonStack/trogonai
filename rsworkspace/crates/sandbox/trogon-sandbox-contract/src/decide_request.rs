use serde::{Deserialize, Serialize};

use crate::{DecideMethod, Host, LeaseId, Port};

/// Question a lease's egress proxy asks the decider about a destination its policy neither allows nor denies.
///
/// `method` is absent when the connection carried no name or request line to learn it from, such as transparent
/// mode without SNI.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct DecideRequest {
    pub lease_id: LeaseId,
    pub host: Host,
    pub port: Port,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<DecideMethod>,
}
