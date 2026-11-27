use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{LeaseId, RequestDecideBody};

/// Question a lease's egress proxy asks about one HTTP request to an admitted destination.
///
/// It mirrors what the egress proxy's decider protocol shows: header names are lower case with their values in order, and the
/// `Host` header is carried apart in `host_header`. `body` is present only on the second ask, after the decider
/// answered [`RequestDecideResponse::NeedBody`](crate::RequestDecideResponse::NeedBody).
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct RequestDecideRequest {
    pub lease_id: LeaseId,
    pub method: String,
    pub host: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_header: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<RequestDecideBody>,
}
