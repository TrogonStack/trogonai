//! ARD catalog host metadata value object.

use serde_json::Value;

use crate::trust_manifest::{TrustManifest, TrustManifestError};

/// Error returned when [`CatalogHost`] validation fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogHostError {
    #[error("host must be a JSON object")]
    NotObject,
    #[error("host.displayName is required")]
    MissingDisplayName,
    #[error("host.displayName must be a string")]
    InvalidDisplayName,
    #[error("host.trustManifest is invalid: {0}")]
    TrustManifest(#[from] TrustManifestError),
}

/// Preserved ARD catalog publisher metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogHost(Value);

impl CatalogHost {
    pub fn new(value: Value) -> Result<Self, CatalogHostError> {
        let object = value.as_object().ok_or(CatalogHostError::NotObject)?;
        match object.get("displayName") {
            Some(Value::String(display_name)) if !display_name.trim().is_empty() => {}
            Some(Value::String(_)) | None => return Err(CatalogHostError::MissingDisplayName),
            Some(_) => return Err(CatalogHostError::InvalidDisplayName),
        }
        if let Some(trust_manifest) = object.get("trustManifest") {
            TrustManifest::new(trust_manifest.clone())?;
        }
        Ok(Self(Value::Object(object.clone())))
    }

    pub fn as_value(&self) -> &Value {
        &self.0
    }

    pub fn into_value(self) -> Value {
        self.0
    }
}

#[cfg(test)]
mod tests;
