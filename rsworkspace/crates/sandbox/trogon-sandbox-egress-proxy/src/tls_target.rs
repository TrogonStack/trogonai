use std::net::Ipv6Addr;

use crate::constants::TLS_DEFAULT_PORT;
use crate::host_mismatch_error::HostMismatchError;
use crate::refusal_error::RefusalError;
use crate::request_version::RequestVersion;

/// The host and port the client's TLS handshake named, which every decrypted
/// request on the connection must also name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TlsTarget {
    host: String,
    port: u16,
}

impl TlsTarget {
    pub fn new(host: &str, port: u16) -> Self {
        Self {
            host: host.to_ascii_lowercase(),
            port,
        }
    }

    pub(crate) fn host(&self) -> &str {
        &self.host
    }

    pub(crate) fn port(&self) -> u16 {
        self.port
    }

    /// The authority `Host` is rebuilt from.
    pub(crate) fn authority(&self) -> String {
        let host = if self.host.parse::<Ipv6Addr>().is_ok() {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == TLS_DEFAULT_PORT {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }

    /// Whether a terminated request names this target. An HTTP/1.0 request
    /// may leave `Host` out.
    pub(crate) fn check(
        &self,
        host_headers: &[&str],
        authority: Option<&str>,
        version: RequestVersion,
    ) -> Result<(), RefusalError> {
        if let Some(authority) = authority
            && !self.names(authority)
        {
            return Err(RefusalError::HostMismatch(HostMismatchError::Authority));
        }
        match host_headers {
            [] if version == RequestVersion::Http10 => Ok(()),
            [] => Err(RefusalError::MissingHost),
            [only] if self.names(only) => Ok(()),
            [_] => Err(RefusalError::HostMismatch(HostMismatchError::Header)),
            _ => Err(RefusalError::HostMismatch(HostMismatchError::MultipleHeaders)),
        }
    }

    fn names(&self, authority: &str) -> bool {
        self.names_by_default(authority, TLS_DEFAULT_PORT)
    }

    fn names_by_default(&self, authority: &str, default_port: u16) -> bool {
        let Some((host, port)) = split_host_and_port(authority) else {
            return false;
        };
        if !host.eq_ignore_ascii_case(&self.host) {
            return false;
        }
        match port {
            "" => self.port == default_port,
            digits => digits == self.port.to_string(),
        }
    }

    /// Whether the `Host` headers of a plain-HTTP request name this target,
    /// whose scheme's default port is `default_port`: none is not a
    /// mismatch, more than one is.
    pub(crate) fn check_plain_hosts(
        &self,
        host_headers: &[String],
        default_port: u16,
    ) -> Result<(), HostMismatchError> {
        match host_headers {
            [] => Ok(()),
            [only] if self.names_by_default(only, default_port) => Ok(()),
            [_] => Err(HostMismatchError::Header),
            _ => Err(HostMismatchError::MultipleHeaders),
        }
    }
}

fn split_host_and_port(authority: &str) -> Option<(&str, &str)> {
    let (host, rest) = if let Some(after_bracket) = authority.strip_prefix('[') {
        let end = after_bracket.find(']')?;
        let host = &after_bracket[..end];
        host.parse::<Ipv6Addr>().ok()?;
        (host, &after_bracket[end + 1..])
    } else {
        let (host, rest) = match authority.find(':') {
            Some(index) => authority.split_at(index),
            None => (authority, ""),
        };
        if host.contains(['[', ']']) {
            return None;
        }
        (host, rest)
    };
    if rest.is_empty() {
        return Some((host, ""));
    }
    let port = rest.strip_prefix(':')?;
    if port.is_empty() || port.contains(['[', ']', ':']) {
        return None;
    }
    Some((host, port))
}

#[cfg(test)]
mod tests;
