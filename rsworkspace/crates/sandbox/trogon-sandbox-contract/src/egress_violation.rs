use std::net::IpAddr;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{EgressDecision, EgressViolationReason, LeaseId, RefusedRequest, SecretName};

/// Structured record of one connection denied by a lease's egress proxy.
///
/// `host` is the sniffed SNI or CONNECT target and is absent when none was available. `dest_ip` is absent when the
/// connection was denied before its host name was resolved. `binding` names the secret binding a
/// `SecretSentinelOffHost` or `SecretCompressedResponse` violation is about, never its value. `at` uses serde's
/// `SystemTime` encoding of `secs_since_epoch` and `nanos_since_epoch`.
#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct EgressViolation {
    pub lease_id: LeaseId,
    pub host: Option<String>,
    pub port: u16,
    pub dest_ip: Option<IpAddr>,
    pub decision: EgressDecision,
    pub reason: EgressViolationReason,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<SecretName>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<RefusedRequest>,
    pub at: SystemTime,
}
