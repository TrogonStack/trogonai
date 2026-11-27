use std::net::IpAddr;

use trogon_sandbox_contract::EgressViolationReason;

use super::{ConnectTarget, IngressOutcome};
use crate::denial::Denial;
use crate::proxy_outcome::ProxyOutcome;

pub(super) fn denied(target: ConnectTarget, dest_ip: Option<IpAddr>, reason: EgressViolationReason) -> IngressOutcome {
    let denial = Denial::new(Some(target.host().to_string()), reason);
    IngressOutcome::Proxied {
        target,
        dest_ip,
        outcome: ProxyOutcome::Denied(denial),
    }
}
