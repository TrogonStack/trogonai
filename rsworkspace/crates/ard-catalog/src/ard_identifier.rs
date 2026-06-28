//! ARD logical identifier (`urn:air:*` only).

use std::sync::Arc;

const URN_AIR_PREFIX: &str = "urn:air:";

/// Error returned when [`ArdIdentifier`] validation fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArdIdentifierError {
    #[error("identifier must not be empty")]
    Empty,
    #[error("identifier must use urn:air:* (found deprecated or invalid urn:ai:* prefix)")]
    DeprecatedAiPrefix,
    #[error("identifier must start with urn:air:")]
    InvalidPrefix,
    #[error("identifier must contain publisher and resource name after urn:air:")]
    TooFewComponents,
    #[error("publisher contains invalid character '{0}'")]
    InvalidPublisherCharacter(char),
    #[error("resource component contains invalid character '{0}'")]
    InvalidResourceCharacter(char),
}

/// Validated ARD logical identifier.
///
/// Accepts `urn:air:<publisher>:<name>` and nested namespace variants.
/// Raw identifiers are not NATS-safe and must not be used as subject tokens.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ArdIdentifier {
    full: Arc<str>,
    publisher_domain: Arc<str>,
    resource_name: Arc<str>,
}

impl ArdIdentifier {
    pub fn new(s: impl AsRef<str>) -> Result<Self, ArdIdentifierError> {
        let s = s.as_ref();
        if s.is_empty() {
            return Err(ArdIdentifierError::Empty);
        }
        if s.starts_with("urn:ai:") || s == "urn:ai" {
            return Err(ArdIdentifierError::DeprecatedAiPrefix);
        }
        if !s.starts_with(URN_AIR_PREFIX) {
            return Err(ArdIdentifierError::InvalidPrefix);
        }
        let remainder = &s[URN_AIR_PREFIX.len()..];
        if remainder.is_empty() {
            return Err(ArdIdentifierError::TooFewComponents);
        }
        let components: Vec<&str> = remainder.split(':').collect();
        if components.len() < 2 || components.iter().any(|part| part.is_empty()) {
            return Err(ArdIdentifierError::TooFewComponents);
        }
        let publisher_domain = components[0];
        if let Some(ch) = publisher_domain
            .chars()
            .find(|ch| !(ch.is_ascii_alphanumeric() || *ch == '.' || *ch == '-'))
        {
            return Err(ArdIdentifierError::InvalidPublisherCharacter(ch));
        }
        for component in components.iter().skip(1) {
            if let Some(ch) = component
                .chars()
                .find(|ch| !(ch.is_ascii_alphanumeric() || *ch == '.' || *ch == '_' || *ch == '-'))
            {
                return Err(ArdIdentifierError::InvalidResourceCharacter(ch));
            }
        }
        let resource_name = components[components.len() - 1];
        Ok(Self {
            full: Arc::from(s),
            publisher_domain: Arc::from(publisher_domain),
            resource_name: Arc::from(resource_name),
        })
    }

    pub fn as_str(&self) -> &str {
        &self.full
    }

    pub fn publisher_domain(&self) -> &str {
        &self.publisher_domain
    }

    pub fn resource_name(&self) -> &str {
        &self.resource_name
    }
}

impl std::fmt::Display for ArdIdentifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.full)
    }
}

#[cfg(test)]
mod tests;
