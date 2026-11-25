use serde::{Deserialize, Serialize};

use crate::{LaunchKey, LeaseId};

/// A request to cancel the command launch recorded under a [`LaunchKey`].
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct CancelCommandLaunchRequest {
    pub lease_id: LeaseId,
    pub launch_key: LaunchKey,
}
