use serde::{Deserialize, Serialize};

use crate::{LeaseError, LeaseSummary};

/// The page of leases answering a [`ListLeasesRequest`](crate::ListLeasesRequest).
///
/// `next_cursor` is omitted on the last page. `denied` is set when the caller may not list the tenant's leases, so a
/// refusal is not mistaken for a tenant without leases. Older brokers never send it, and it is omitted when absent.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct ListLeasesResponse {
    pub leases: Vec<LeaseSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denied: Option<LeaseError>,
}
