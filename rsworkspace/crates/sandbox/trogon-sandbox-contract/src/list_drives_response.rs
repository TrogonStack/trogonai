use serde::{Deserialize, Serialize};

use crate::{DriveError, DriveRecord};

/// The drives answering a [`ListDrivesRequest`](crate::ListDrivesRequest).
///
/// `denied` is set when the caller may not list the tenant's drives, so a refusal is not mistaken for a tenant without
/// drives. Older brokers never send it, and it is omitted when absent.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListDrivesResponse {
    pub drives: Vec<DriveRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denied: Option<DriveError>,
}
