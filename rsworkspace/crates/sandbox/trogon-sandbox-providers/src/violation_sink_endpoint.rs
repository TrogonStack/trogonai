mod constants;

use std::net::Ipv4Addr;

use crate::{SinkHost, ViolationSinkEndpointError};
use constants::DEFAULT_NATS_PORT;

/// One NATS server the egress proxy must be able to reach to publish events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViolationSinkEndpoint {
    pub host: SinkHost,
    pub port: u16,
}

impl ViolationSinkEndpoint {
    /// Parses a NATS URL, or a comma separated list of them, into endpoints.
    /// Anything that is not a plain IPv4 literal or DNS name with an optional
    /// port is rejected, so no iptables exemption is ever built from it.
    pub fn parse_all(nats_url: &str) -> Result<Vec<Self>, ViolationSinkEndpointError> {
        nats_url
            .split(',')
            .map(|entry| Self::parse_one(entry.trim()).ok_or_else(|| ViolationSinkEndpointError(nats_url.to_string())))
            .collect()
    }

    fn parse_one(entry: &str) -> Option<Self> {
        let rest = entry.split_once("://").map_or(entry, |(_, rest)| rest);
        let authority = rest.split('/').next()?;
        let hostport = authority.rsplit_once('@').map_or(authority, |(_, host)| host);
        let (host, port) = match hostport.split_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().ok().filter(|port| *port != 0)?),
            None => (hostport, DEFAULT_NATS_PORT),
        };
        if host.is_empty() {
            return None;
        }
        let host = match host.parse::<Ipv4Addr>() {
            Ok(ip) => SinkHost::Ipv4(ip),
            Err(_) => {
                let valid = host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_');
                if !valid {
                    return None;
                }
                SinkHost::Name(host.to_string())
            }
        };
        Some(Self { host, port })
    }
}
