use super::constants::MAX_DNS1123_LABEL;
use super::dns_label_error::DnsLabelError;

pub(super) fn validate_dns1123_label(value: &str) -> Result<(), DnsLabelError> {
    let (Some(first), Some(last)) = (value.chars().next(), value.chars().next_back()) else {
        return Err(DnsLabelError::Empty);
    };
    if value.len() > MAX_DNS1123_LABEL {
        return Err(DnsLabelError::TooLong { length: value.len() });
    }
    if let Some(character) = value
        .chars()
        .find(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-'))
    {
        return Err(DnsLabelError::InvalidCharacter { character });
    }
    if !first.is_ascii_alphanumeric() {
        return Err(DnsLabelError::BadStart);
    }
    if !last.is_ascii_alphanumeric() {
        return Err(DnsLabelError::BadEnd);
    }
    Ok(())
}
