use std::fmt;
use std::net::Ipv6Addr;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{DomainPatternError, Host, PatternTarget, Port};

/// A destination pattern with an optional port.
///
/// It is `example.com`, `*.example.com` (strict subdomains only, never IP literals), or an IP literal such as
/// `10.0.0.1` or `[::1]`, each optionally followed by `:port`. A pattern without a port covers every port. A bare
/// `*` is not a pattern; only a [`DeniedDomain`](crate::DeniedDomain) accepts it. A single-label name must be
/// `localhost`. It serializes as its canonical string, and construction, parsing, and deserialization all enforce
/// the grammar.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DomainPattern {
    target: PatternTarget,
    port: Option<Port>,
}

impl DomainPattern {
    pub fn new(input: &str) -> Result<Self, DomainPatternError> {
        Self::parse_with(input, false)
    }

    pub fn target(&self) -> &PatternTarget {
        &self.target
    }

    pub fn port(&self) -> Option<Port> {
        self.port
    }

    /// Parses `input`, accepting a bare `*` only when `allow_any` is set.
    pub(crate) fn parse_with(input: &str, allow_any: bool) -> Result<Self, DomainPatternError> {
        if input.is_empty() {
            return Err(DomainPatternError::Empty);
        }
        let (host_part, port) = split_port(input)?;

        if host_part == "*" {
            return if allow_any {
                Ok(Self {
                    target: PatternTarget::Any,
                    port,
                })
            } else {
                Err(DomainPatternError::BareWildcardNotAllowed)
            };
        }

        if let Some(suffix) = host_part.strip_prefix("*.") {
            let invalid = || DomainPatternError::InvalidWildcard(input.to_string());
            if suffix.contains('*') {
                return Err(invalid());
            }
            let host = Host::new(suffix).map_err(|source| DomainPatternError::InvalidWildcardHost {
                input: input.to_string(),
                source,
            })?;
            return match host.as_domain() {
                Some(name) if name.as_str().contains('.') => Ok(Self {
                    target: PatternTarget::Subdomains(name.clone()),
                    port,
                }),
                _ => Err(invalid()),
            };
        }

        if host_part.contains('*') {
            return Err(DomainPatternError::InvalidWildcard(input.to_string()));
        }

        let host = Host::new(&host_part)?;
        if let Some(name) = host.as_domain()
            && !name.as_str().contains('.')
            && name.as_str() != "localhost"
        {
            return Err(DomainPatternError::SingleLabel(input.to_string()));
        }
        Ok(Self {
            target: PatternTarget::Exact(host),
            port,
        })
    }
}

fn split_port(input: &str) -> Result<(String, Option<Port>), DomainPatternError> {
    if let Some(rest) = input.strip_prefix('[') {
        let malformed = || DomainPatternError::MalformedBrackets(input.to_string());
        let (inner, after) = rest.split_once(']').ok_or_else(malformed)?;
        let address = inner.split_once('%').map_or(inner, |(address, _zone)| address);
        if address.parse::<Ipv6Addr>().is_err() {
            return Err(malformed());
        }
        let port = match after {
            "" => None,
            after => Some(after.strip_prefix(':').ok_or_else(malformed)?.parse::<Port>()?),
        };
        return Ok((format!("[{address}]"), port));
    }

    let mut colons = input.match_indices(':');
    match (colons.next(), colons.next()) {
        (None, _) => Ok((input.to_string(), None)),
        (Some((index, _)), None) => Ok((input[..index].to_string(), Some(input[index + 1..].parse()?))),
        _ => Err(DomainPatternError::UnbracketedIpv6(input.to_string())),
    }
}

impl TryFrom<String> for DomainPattern {
    type Error = DomainPatternError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(&value)
    }
}

impl FromStr for DomainPattern {
    type Err = DomainPatternError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl From<DomainPattern> for String {
    fn from(pattern: DomainPattern) -> Self {
        pattern.to_string()
    }
}

impl fmt::Display for DomainPattern {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let target = match &self.target {
            PatternTarget::Any => "*".to_string(),
            PatternTarget::Exact(host) => host.to_string(),
            PatternTarget::Subdomains(suffix) => format!("*.{suffix}"),
        };
        match self.port {
            Some(port) => write!(formatter, "{target}:{port}"),
            None => formatter.write_str(&target),
        }
    }
}
