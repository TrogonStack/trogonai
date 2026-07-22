//! Typed facet field value object for explore requests.

/// Error returned when [`FacetField`] parsing fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FacetFieldError {
    #[error("unsupported facet field: {0}")]
    Unsupported(String),
}

/// Typed enumeration of valid ARD registry facet fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FacetField {
    Type,
    Tags,
    Capabilities,
}

impl FacetField {
    pub fn parse(s: &str) -> Result<Self, FacetFieldError> {
        match s {
            "type" => Ok(Self::Type),
            "tags" => Ok(Self::Tags),
            "capabilities" => Ok(Self::Capabilities),
            other => Err(FacetFieldError::Unsupported(other.to_owned())),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::Tags => "tags",
            Self::Capabilities => "capabilities",
        }
    }
}

#[cfg(test)]
mod tests;
