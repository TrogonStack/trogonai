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

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("allowed host must be a DNS name, IP address, or host:port")]
pub struct AllowedHostError;

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
            let colon = raw.find(':').unwrap_or(raw.len());
            let (host, port) = raw.split_at(colon);
            (host, Some(&port[1..]))
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
mod tests;
