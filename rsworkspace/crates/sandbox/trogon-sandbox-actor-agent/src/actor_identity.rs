use serde::{Deserialize, Serialize};

use crate::ActorId;

/// Body of the identity endpoint.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActorIdentity {
    pub actor_id: ActorId,
}
