use std::fmt;

/// The host, and optional port, clients reach the edge at. Every signature
/// covers it as `@authority`, so a request signed for another host does not
/// verify here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EdgeAuthority(String);

impl EdgeAuthority {
    /// # Errors
    ///
    /// [`EdgeAuthorityError`] when `raw` is empty or carries anything beyond
    /// a host and port, such as a scheme, path or user info.
    pub fn new(raw: impl AsRef<str>) -> Result<Self, EdgeAuthorityError> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty() {
            return Err(EdgeAuthorityError::Empty);
        }
        if trimmed
            .chars()
            .any(|ch| ch.is_whitespace() || matches!(ch, '/' | '?' | '#' | '@'))
        {
            return Err(EdgeAuthorityError::NotAnAuthority(trimmed.to_owned()));
        }
        Ok(Self(trimmed.to_ascii_lowercase()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EdgeAuthority {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EdgeAuthorityError {
    #[error("authority is empty")]
    Empty,
    #[error("{0:?} is not a host with an optional port")]
    NotAnAuthority(String),
}
