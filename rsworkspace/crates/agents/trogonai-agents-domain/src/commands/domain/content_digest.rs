use std::fmt;
use std::str::FromStr;

const PREFIX: &str = "sha256:";
const HEX_LENGTH: usize = 64;
const ENCODED_LENGTH: usize = PREFIX.len() + HEX_LENGTH;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentDigest(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContentDigestError {
    #[error("content digest must start with 'sha256:'")]
    InvalidPrefix,
    #[error("content digest must contain exactly 64 lowercase hexadecimal digits, got {actual}")]
    InvalidLength { actual: usize },
    #[error("content digest contains invalid hexadecimal character '{character}' at index {index}")]
    InvalidHex { index: usize, character: char },
}

impl ContentDigest {
    pub fn parse(raw: &str) -> Result<Self, ContentDigestError> {
        let Some(hex) = raw.strip_prefix(PREFIX) else {
            return Err(ContentDigestError::InvalidPrefix);
        };
        if raw.len() != ENCODED_LENGTH {
            return Err(ContentDigestError::InvalidLength {
                actual: hex.chars().count(),
            });
        }
        if let Some((index, character)) = hex
            .char_indices()
            .find(|(_, character)| !matches!(character, '0'..='9' | 'a'..='f'))
        {
            return Err(ContentDigestError::InvalidHex { index, character });
        }
        Ok(Self(raw.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl AsRef<str> for ContentDigest {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl FromStr for ContentDigest {
    type Err = ContentDigestError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

impl fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
