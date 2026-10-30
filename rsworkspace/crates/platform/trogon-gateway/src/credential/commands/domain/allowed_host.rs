use std::collections::BTreeSet;
use std::fmt;
use std::net::Ipv6Addr;

use super::constants::MAX_HOST_LEN;

/// One entry in a credential's allowed-host list.
///
/// Either an exact host (`api.example.com`) or a single-label wildcard
/// (`*.example.com`), following the TLS certificate convention: `*.example.com`
/// matches `api.example.com` but neither `example.com` itself nor
/// `a.b.example.com`.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum AllowedHost {
    Exact(String),
    WildcardSubdomain(String),
}

impl AllowedHost {
    pub fn new(value: impl AsRef<str>) -> Result<Self, AllowedHostError> {
        let value = value.as_ref().trim();
        if value.is_empty() {
            return Err(AllowedHostError::Empty);
        }
        if value.len() > MAX_HOST_LEN {
            return Err(AllowedHostError::TooLong(value.len()));
        }

        let normalized = normalize_host(value);
        if let Some(literal) = unbracketed_ipv6_literal(&normalized) {
            return Ok(Self::Exact(literal));
        }
        if normalized.starts_with('[') {
            let address = validate_ipv6_literal(&normalized)?;
            return Ok(Self::Exact(bracketed(address)));
        }
        if has_port(&normalized) {
            return Err(AllowedHostError::PortNotAllowed);
        }
        if normalized == "*" {
            return Err(AllowedHostError::BareWildcard);
        }
        if let Some(suffix) = normalized.strip_prefix("*.") {
            if suffix.is_empty() {
                return Err(AllowedHostError::BareWildcard);
            }
            validate_labels(suffix)?;
            return Ok(Self::WildcardSubdomain(suffix.to_string()));
        }

        validate_labels(&normalized)?;
        Ok(Self::Exact(normalized))
    }

    /// `candidate` is matched after the same normalization the pattern went
    /// through, so a trailing-dot FQDN, an uppercase host, or a `host:port`
    /// authority cannot slip past a rule that would otherwise deny it.
    pub fn matches(&self, candidate: &str) -> bool {
        let Some(candidate) = normalize_candidate(candidate) else {
            return false;
        };
        match self {
            Self::Exact(host) => *host == candidate,
            Self::WildcardSubdomain(suffix) => match candidate.strip_suffix(suffix) {
                Some(label) => label.ends_with('.') && !label[..label.len() - 1].contains('.') && label.len() > 1,
                None => false,
            },
        }
    }

    pub fn as_str(&self) -> String {
        match self {
            Self::Exact(host) => host.clone(),
            Self::WildcardSubdomain(suffix) => format!("*.{suffix}"),
        }
    }
}

impl fmt::Display for AllowedHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_str())
    }
}

/// The host restriction on a credential.
///
/// `Unrestricted` is a distinct variant rather than an empty set so that "no
/// policy has been configured" and "this credential may be sent nowhere" stay
/// different states. An empty `Only` set denies everything.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum AllowedHosts {
    #[default]
    Unrestricted,
    Only(BTreeSet<AllowedHost>),
}

impl AllowedHosts {
    pub fn only<I, T>(hosts: I) -> Result<Self, AllowedHostError>
    where
        I: IntoIterator<Item = T>,
        T: AsRef<str>,
    {
        let hosts = hosts
            .into_iter()
            .map(AllowedHost::new)
            .collect::<Result<BTreeSet<_>, _>>()?;
        Ok(Self::Only(hosts))
    }

    pub fn is_unrestricted(&self) -> bool {
        matches!(self, Self::Unrestricted)
    }

    /// Fails closed: when a restriction exists and no host was supplied by the
    /// caller, the answer is deny, not allow.
    pub fn permits(&self, candidate: Option<&str>) -> bool {
        match self {
            Self::Unrestricted => true,
            Self::Only(hosts) => match candidate {
                Some(candidate) => hosts.iter().any(|host| host.matches(candidate)),
                None => false,
            },
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum AllowedHostError {
    #[error("allowed host must not be empty")]
    Empty,
    #[error("allowed host exceeds maximum length: {0}")]
    TooLong(usize),
    #[error("allowed host '*' must name a parent domain, as in '*.example.com'")]
    BareWildcard,
    #[error("allowed host label must not be empty")]
    EmptyLabel,
    #[error("allowed host contains invalid character '{0}'")]
    InvalidCharacter(char),
    #[error("allowed host may only use '*' as a leading label")]
    MisplacedWildcard,
    #[error("allowed host must not include a port because ports are ignored when matching")]
    PortNotAllowed,
    #[error("allowed host in brackets must be an IPv6 address, as in '[::1]'")]
    InvalidIpv6Literal,
}

fn normalize_host(value: &str) -> String {
    let lowered = value.to_ascii_lowercase();
    lowered.strip_suffix('.').unwrap_or(&lowered).to_string()
}

/// Strips the port from an authority so `example.com:8443` is matched against
/// the `example.com` rule. IPv6 literals keep their bracketed form.
fn normalize_candidate(candidate: &str) -> Option<String> {
    let candidate = candidate.trim();
    if candidate.is_empty() {
        return None;
    }

    if let Some(literal) = unbracketed_ipv6_literal(&normalize_host(candidate)) {
        return Some(literal);
    }

    if let Some(inner) = candidate.strip_prefix('[') {
        let literal = inner
            .split_once(']')
            .filter(|(_, rest)| rest.is_empty() || rest.strip_prefix(':').is_some_and(is_port))
            .and_then(|(address, _)| address.parse::<Ipv6Addr>().ok());
        return literal.map(bracketed);
    }

    let without_port = match candidate.rsplit_once(':') {
        Some((host, port)) if is_port(port) => host,
        _ => candidate,
    };

    let normalized = normalize_host(without_port);
    validate_labels(&normalized).ok().map(|()| normalized)
}

fn has_port(value: &str) -> bool {
    value.split_once(':').is_some_and(|(_, port)| is_port(port))
}

fn is_port(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

fn bracketed(address: Ipv6Addr) -> String {
    format!("[{address}]")
}

fn unbracketed_ipv6_literal(value: &str) -> Option<String> {
    value.parse::<Ipv6Addr>().ok().map(bracketed)
}

fn validate_ipv6_literal(value: &str) -> Result<Ipv6Addr, AllowedHostError> {
    let (address, rest) = value
        .strip_prefix('[')
        .and_then(|inner| inner.split_once(']'))
        .ok_or(AllowedHostError::InvalidIpv6Literal)?;
    if has_port(rest) {
        return Err(AllowedHostError::PortNotAllowed);
    }
    if !rest.is_empty() {
        return Err(AllowedHostError::InvalidIpv6Literal);
    }
    address.parse().map_err(|_| AllowedHostError::InvalidIpv6Literal)
}

fn validate_labels(value: &str) -> Result<(), AllowedHostError> {
    if value.is_empty() {
        return Err(AllowedHostError::Empty);
    }
    for label in value.split('.') {
        if label.is_empty() {
            return Err(AllowedHostError::EmptyLabel);
        }
        for ch in label.chars() {
            if ch == '*' {
                return Err(AllowedHostError::MisplacedWildcard);
            }
            if !is_host_char(ch) {
                return Err(AllowedHostError::InvalidCharacter(ch));
            }
        }
    }
    Ok(())
}

fn is_host_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')
}

#[cfg(test)]
mod tests;
