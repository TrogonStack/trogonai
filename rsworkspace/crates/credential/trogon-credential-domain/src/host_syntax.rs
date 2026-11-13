use std::net::Ipv6Addr;

/// Host syntax faults shared by every host value object.
///
/// Each public host type maps these onto its own error so that a fault which
/// makes no sense for that type, such as a misplaced wildcard on a type that
/// never accepts one, does not leak into its contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum HostSyntaxError {
    Empty,
    EmptyLabel,
    InvalidCharacter(char),
    MisplacedWildcard,
    PortNotAllowed,
    InvalidIpv6Literal,
}

pub(super) fn normalize_host(value: &str) -> String {
    let lowered = value.to_ascii_lowercase();
    lowered.strip_suffix('.').unwrap_or(&lowered).to_string()
}

pub(super) fn has_port(value: &str) -> bool {
    value.split_once(':').is_some_and(|(_, port)| is_port(port))
}

pub(super) fn is_port(value: &str) -> bool {
    !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit())
}

pub(super) fn bracketed(address: Ipv6Addr) -> String {
    format!("[{address}]")
}

pub(super) fn unbracketed_ipv6_literal(value: &str) -> Option<String> {
    value.parse::<Ipv6Addr>().ok().map(bracketed)
}

pub(super) fn validate_ipv6_literal(value: &str) -> Result<Ipv6Addr, HostSyntaxError> {
    let (address, rest) = value
        .strip_prefix('[')
        .and_then(|inner| inner.split_once(']'))
        .ok_or(HostSyntaxError::InvalidIpv6Literal)?;
    if has_port(rest) {
        return Err(HostSyntaxError::PortNotAllowed);
    }
    if !rest.is_empty() {
        return Err(HostSyntaxError::InvalidIpv6Literal);
    }
    address.parse().map_err(|_| HostSyntaxError::InvalidIpv6Literal)
}

pub(super) fn validate_labels(value: &str) -> Result<(), HostSyntaxError> {
    if value.is_empty() {
        return Err(HostSyntaxError::Empty);
    }
    for label in value.split('.') {
        if label.is_empty() {
            return Err(HostSyntaxError::EmptyLabel);
        }
        for ch in label.chars() {
            if ch == '*' {
                return Err(HostSyntaxError::MisplacedWildcard);
            }
            if !is_host_char(ch) {
                return Err(HostSyntaxError::InvalidCharacter(ch));
            }
        }
    }
    Ok(())
}

fn is_host_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_')
}
