use serde::{Deserialize, Serialize};
use trogon_sandbox_contract::WorkerId;

use super::{ActorClaimId, ActorUid};

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActorOwner {
    pub worker_id: WorkerId,
    pub claim_id: ActorClaimId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_uid: Option<ActorUid>,
}
