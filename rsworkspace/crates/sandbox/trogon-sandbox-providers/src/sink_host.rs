use std::net::Ipv4Addr;

/// The host of a violation sink: an IPv4 literal or a DNS name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SinkHost {
    Ipv4(Ipv4Addr),
    Name(String),
}
