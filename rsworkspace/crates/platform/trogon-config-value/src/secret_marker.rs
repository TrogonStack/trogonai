use crate::constants::SECRET_SCHEME;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SecretMarkerError {
    #[error("secret namespace identifier must only contain lowercase letters, digits, and '-'")]
    InvalidNamespaceId,
    #[error("secret namespace specific string must only contain lowercase letters, digits, and '-'")]
    InvalidNamespaceSpecific,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecretMarker {
    namespace_id: String,
    namespace_specific: String,
}

impl SecretMarker {
    pub fn new(
        namespace_id: impl Into<String>,
        namespace_specific: impl Into<String>,
    ) -> Result<Self, SecretMarkerError> {
        let namespace_id = namespace_id.into();
        let namespace_specific = namespace_specific.into();
        if !is_identifier(&namespace_id) {
            return Err(SecretMarkerError::InvalidNamespaceId);
        }
        if !is_identifier(&namespace_specific) {
            return Err(SecretMarkerError::InvalidNamespaceSpecific);
        }
        Ok(Self {
            namespace_id,
            namespace_specific,
        })
    }

    pub fn namespace_id(&self) -> &str {
        &self.namespace_id
    }

    pub fn namespace_specific(&self) -> &str {
        &self.namespace_specific
    }

    pub(crate) fn split(raw: &str) -> Option<(Self, &str)> {
        let rest = raw.strip_prefix(SECRET_SCHEME)?;
        let mut parts = rest.splitn(3, ':');
        let namespace_id = parts.next()?;
        let namespace_specific = parts.next()?;
        let value = parts.next().filter(|value| !value.is_empty())?;
        let marker = Self::new(namespace_id, namespace_specific).ok()?;
        Some((marker, value))
    }
}

fn is_identifier(part: &str) -> bool {
    part.bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

#[cfg(test)]
mod tests;
