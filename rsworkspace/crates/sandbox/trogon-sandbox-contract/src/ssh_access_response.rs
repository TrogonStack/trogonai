use serde::{Deserialize, Serialize};

use crate::{LeaseError, SshAccess};

/// The reply to a [`SshAccessRequest`](crate::SshAccessRequest), tagged by `outcome`.
///
/// `Unsupported` means the provider has no SSH endpoint of its own to hand out; such leases are reached through the
/// SSH gateway instead.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum SshAccessResponse {
    Available(SshAccess),
    Unsupported,
    Denied(LeaseError),
}
