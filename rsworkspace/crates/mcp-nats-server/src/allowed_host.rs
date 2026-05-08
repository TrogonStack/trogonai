#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AllowedHost(String);

impl AllowedHost {
    pub fn new(raw: impl Into<String>) -> Result<Self, AllowedHostError> {
        let raw = raw.into();
        if !is_valid_allowed_host(&raw) {
            return Err(AllowedHostError);
        }
        Ok(Self(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllowedHostError;

impl std::fmt::Display for AllowedHostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "allowed host must be a DNS name, IP address, or host:port")
    }
}

impl std::error::Error for AllowedHostError {}

fn is_valid_allowed_host(raw: &str) -> bool {
    if raw.is_empty()
        || raw
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '?' | '#' | '@'))
    {
        return false;
    }

    if raw.starts_with('[') {
        return is_valid_bracketed_ip(raw);
    }

    let colon_count = raw.bytes().filter(|byte| *byte == b':').count();
    let (host, port) = match colon_count {
        0 => (raw, None),
        1 => {
            let (host, port) = raw.rsplit_once(':').expect("single colon is present");
            (host, Some(port))
        }
        _ => return false,
    };

    !host.is_empty()
        && port.is_none_or(is_valid_port)
        && (host.parse::<std::net::IpAddr>().is_ok() || is_valid_dns_name(host))
}

fn is_valid_bracketed_ip(raw: &str) -> bool {
    let Some((address, rest)) = raw[1..].split_once(']') else {
        return false;
    };
    if address.parse::<std::net::IpAddr>().is_err() {
        return false;
    }
    rest.is_empty() || rest.strip_prefix(':').is_some_and(is_valid_port)
}

fn is_valid_port(port: &str) -> bool {
    !port.is_empty() && port.parse::<u16>().is_ok()
}

fn is_valid_dns_name(host: &str) -> bool {
    host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
                && !label.starts_with('-')
                && !label.ends_with('-')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_dns_ip_and_port_forms() {
        for raw in [
            "localhost",
            "example.com",
            "api.example.com:8443",
            "127.0.0.1",
            "127.0.0.1:8081",
            "[::1]",
            "[::1]:8081",
        ] {
            assert_eq!(AllowedHost::new(raw).unwrap().as_str(), raw);
        }
    }

    #[test]
    fn rejects_empty_unsafe_or_malformed_hosts() {
        for raw in [
            "",
            "example com",
            "example.com/path",
            "example.com?x=1",
            "example.com#fragment",
            "user@example.com",
            "-example.com",
            "example-.com",
            "example..com",
            "example.com:",
            "example.com:bad",
            "::1",
            "[not-an-ip]",
            "[::1",
            "[::1]bad",
        ] {
            assert!(AllowedHost::new(raw).is_err(), "{raw} should be invalid");
        }
    }

    #[test]
    fn error_display_is_specific() {
        assert_eq!(
            AllowedHostError.to_string(),
            "allowed host must be a DNS name, IP address, or host:port"
        );
    }
}
