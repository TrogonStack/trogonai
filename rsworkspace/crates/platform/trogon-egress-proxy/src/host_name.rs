use crate::constants::{HOST_NAME_MAX_LABEL_LEN, HOST_NAME_MAX_LEN};
use std::fmt;

/// An exact DNS name, ASCII only and folded to lowercase.
///
/// This is the name side of an `EgressHost`: never a pattern, never an address.
/// A name whose last label does not start with a letter is refused, because
/// resolvers read `127.1`, `2130706433` and `0x7f.1` as IPv4 addresses and a
/// permit written as a "name" must not be a spelling of an address.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HostName(String);

impl HostName {
    pub fn new(raw: impl AsRef<str>) -> Result<Self, HostNameError> {
        let raw = raw.as_ref();
        if raw.is_empty() {
            return Err(HostNameError::Empty);
        }
        if raw.len() > HOST_NAME_MAX_LEN {
            return Err(HostNameError::TooLong { max: HOST_NAME_MAX_LEN });
        }
        if let Some(character) = raw.chars().find(|c| !is_allowed(*c)) {
            return Err(HostNameError::InvalidCharacter { character });
        }
        let lowered = raw.to_ascii_lowercase();
        for label in lowered.split('.') {
            if label.is_empty() {
                return Err(HostNameError::EmptyLabel);
            }
            if label.len() > HOST_NAME_MAX_LABEL_LEN {
                return Err(HostNameError::LabelTooLong {
                    max: HOST_NAME_MAX_LABEL_LEN,
                });
            }
            if label.starts_with('-') || label.ends_with('-') {
                return Err(HostNameError::HyphenAtLabelEdge);
            }
        }
        let starts_with_letter = lowered
            .rsplit('.')
            .next()
            .and_then(|last| last.chars().next())
            .is_some_and(|c| c.is_ascii_lowercase());
        if !starts_with_letter {
            return Err(HostNameError::LooksLikeAddress);
        }
        Ok(Self(lowered))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
}

impl fmt::Display for HostName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum HostNameError {
    #[error("host must not be empty")]
    Empty,
    #[error("host must not be longer than {max} bytes")]
    TooLong { max: usize },
    #[error("host must not contain {character:?}")]
    InvalidCharacter { character: char },
    #[error("host must not have an empty label")]
    EmptyLabel,
    #[error("host labels must not be longer than {max} bytes")]
    LabelTooLong { max: usize },
    #[error("host labels must not start or end with a hyphen")]
    HyphenAtLabelEdge,
    #[error("host's last label must start with a letter so it cannot be read as an address")]
    LooksLikeAddress,
}

#[cfg(test)]
mod tests;
