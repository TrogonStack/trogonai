use std::io;
use std::net::IpAddr;

use trogon_sandbox_contract::{DenyReason, DomainRuleAction, EgressViolationReason};

use crate::resolve::VettedAddr;

pub(super) enum Admission {
    Allowed {
        dest: VettedAddr,
        fallbacks: Vec<VettedAddr>,
        action: Option<DomainRuleAction>,
    },
    Denied {
        dest_ip: Option<IpAddr>,
        reason: EgressViolationReason,
        explanation: Option<DenyReason>,
    },
    Unresolved(io::Error),
}
