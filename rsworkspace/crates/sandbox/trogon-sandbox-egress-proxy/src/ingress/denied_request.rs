use std::net::IpAddr;

use trogon_sandbox_contract::{EgressViolationReason, RefusedRequest};

use super::{ConnectTarget, IngressOutcome};
use crate::denial::Denial;
use crate::proxy_outcome::ProxyOutcome;

pub(super) fn denied_request(
    target: ConnectTarget,
    dest_ip: Option<IpAddr>,
    request: RefusedRequest,
) -> IngressOutcome {
    let denial = Denial::new(Some(target.host().to_string()), EgressViolationReason::RequestDenied).refusing(request);
    IngressOutcome::Proxied {
        target,
        dest_ip,
        outcome: ProxyOutcome::Denied(denial),
    }
}
