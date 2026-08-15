use std::collections::BTreeSet;
use std::fmt;

const MAX_HOST_LEN: usize = 253;

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

    let without_port = if let Some(end) = candidate.strip_prefix('[').and_then(|rest| rest.find(']')) {
        &candidate[..end + 2]
    } else {
        match candidate.rsplit_once(':') {
            Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
            _ => candidate,
        }
    };

    let normalized = normalize_host(without_port);
    if normalized.is_empty() { None } else { Some(normalized) }
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
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '[' | ']' | ':')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_host_matches_itself_only() {
        let host = AllowedHost::new("api.example.com").unwrap();

        assert!(host.matches("api.example.com"));
        assert!(!host.matches("example.com"));
        assert!(!host.matches("evil.com"));
    }

    #[test]
    fn matching_is_case_insensitive_and_ignores_the_root_dot() {
        let host = AllowedHost::new("API.Example.COM").unwrap();

        assert!(host.matches("api.example.com"));
        assert!(host.matches("API.EXAMPLE.COM"));
        assert!(host.matches("api.example.com."));
    }

    #[test]
    fn port_is_stripped_before_matching() {
        let host = AllowedHost::new("api.example.com").unwrap();

        assert!(host.matches("api.example.com:8443"));
        assert!(!host.matches("api.example.com:notaport"));
    }

    #[test]
    fn wildcard_matches_exactly_one_label() {
        let host = AllowedHost::new("*.example.com").unwrap();

        assert!(host.matches("api.example.com"));
        assert!(!host.matches("example.com"));
        assert!(!host.matches("a.b.example.com"));
    }

    #[test]
    fn wildcard_does_not_match_a_suffix_that_is_not_a_label_boundary() {
        let host = AllowedHost::new("*.example.com").unwrap();

        assert!(!host.matches("notexample.com"));
        assert!(!host.matches("evilexample.com"));
    }

    #[test]
    fn rejects_malformed_patterns() {
        assert_eq!(AllowedHost::new(""), Err(AllowedHostError::Empty));
        assert_eq!(AllowedHost::new("*"), Err(AllowedHostError::BareWildcard));
        assert_eq!(AllowedHost::new("*."), Err(AllowedHostError::BareWildcard));
        assert_eq!(
            AllowedHost::new("api.*.example.com"),
            Err(AllowedHostError::MisplacedWildcard)
        );
        assert_eq!(AllowedHost::new("api..example.com"), Err(AllowedHostError::EmptyLabel));
        assert_eq!(
            AllowedHost::new("api.example.com/path"),
            Err(AllowedHostError::InvalidCharacter('/'))
        );
    }

    #[test]
    fn unrestricted_permits_anything_including_an_absent_host() {
        let hosts = AllowedHosts::Unrestricted;

        assert!(hosts.permits(Some("anything.example.com")));
        assert!(hosts.permits(None));
    }

    #[test]
    fn restricted_denies_an_absent_host() {
        let hosts = AllowedHosts::only(["api.example.com"]).unwrap();

        assert!(hosts.permits(Some("api.example.com")));
        assert!(!hosts.permits(None));
    }

    #[test]
    fn empty_allow_list_denies_everything() {
        let hosts = AllowedHosts::Only(BTreeSet::new());

        assert!(!hosts.permits(Some("api.example.com")));
        assert!(!hosts.permits(None));
    }

    #[test]
    fn round_trips_through_display() {
        assert_eq!(AllowedHost::new("*.example.com").unwrap().to_string(), "*.example.com");
        assert_eq!(
            AllowedHost::new("api.example.com").unwrap().to_string(),
            "api.example.com"
        );
    }
}
