use std::io;

use super::{ConnectTarget, IngressOutcome};
use crate::proxy_outcome::ProxyOutcome;

pub(super) fn unreachable(target: ConnectTarget, err: io::Error) -> IngressOutcome {
    IngressOutcome::Proxied {
        target,
        dest_ip: None,
        outcome: ProxyOutcome::UpstreamConnectFailed(err),
    }
}
