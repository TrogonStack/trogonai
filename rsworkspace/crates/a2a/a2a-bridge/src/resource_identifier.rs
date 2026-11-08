use std::fmt;

/// The edge's own identifier: the audience an auth token has to name and the
/// issuer of the nonces the edge hands out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceIdentifier(String);

impl ResourceIdentifier {
    /// # Errors
    ///
    /// [`EmptyResourceIdentifierError`] when `raw` is empty.
    pub fn new(raw: impl AsRef<str>) -> Result<Self, EmptyResourceIdentifierError> {
        let trimmed = raw.as_ref().trim();
        if trimmed.is_empty() {
            return Err(EmptyResourceIdentifierError);
        }
        Ok(Self(trimmed.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ResourceIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("resource identifier is empty")]
pub struct EmptyResourceIdentifierError;
