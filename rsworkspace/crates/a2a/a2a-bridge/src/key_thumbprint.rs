use std::fmt;

/// RFC 7638 thumbprint of the key a caller signed its request with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyThumbprint(String);

impl KeyThumbprint {
    pub(crate) fn new(value: String) -> Self {
        Self(value)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for KeyThumbprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}
