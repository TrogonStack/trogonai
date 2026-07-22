pub(crate) fn valid_qualified_name(value: &str) -> bool {
    let mut parts = value.split('/');
    let first = parts.next().unwrap_or_default();
    let second = parts.next();
    if parts.next().is_some() {
        return false;
    }
    match second {
        Some(name) => valid_dns_subdomain(first) && valid_name_part(name),
        None => valid_name_part(first),
    }
}

pub(crate) fn valid_label_value(value: &str) -> bool {
    value.is_empty() || valid_name_part(value)
}

fn valid_name_part(value: &str) -> bool {
    if value.is_empty() || value.len() > 63 || !value.is_ascii() {
        return false;
    }
    let bytes = value.as_bytes();
    is_ascii_alphanumeric(bytes[0])
        && is_ascii_alphanumeric(bytes[bytes.len() - 1])
        && bytes
            .iter()
            .all(|byte| is_ascii_alphanumeric(*byte) || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_dns_subdomain(value: &str) -> bool {
    !value.is_empty() && value.len() <= 253 && value.split('.').all(valid_dns_label)
}

fn valid_dns_label(value: &str) -> bool {
    if value.is_empty() || value.len() > 63 || !value.is_ascii() {
        return false;
    }
    let bytes = value.as_bytes();
    is_ascii_lower_alphanumeric(bytes[0])
        && is_ascii_lower_alphanumeric(bytes[bytes.len() - 1])
        && bytes
            .iter()
            .all(|byte| is_ascii_lower_alphanumeric(*byte) || *byte == b'-')
}

const fn is_ascii_alphanumeric(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

const fn is_ascii_lower_alphanumeric(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit()
}
