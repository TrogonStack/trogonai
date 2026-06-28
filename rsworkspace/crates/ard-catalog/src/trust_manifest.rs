//! ARD trust manifest metadata preservation.

use serde_json::Value;

/// Error returned when [`TrustManifest`] validation fails.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TrustManifestError {
    #[error("trustManifest must be a JSON object")]
    NotObject,
    #[error("trustManifest.identity is required")]
    MissingIdentity,
    #[error("trustManifest.identity must be a string")]
    InvalidIdentity,
    #[error("trustManifest.identityType is not supported")]
    InvalidIdentityType,
}

/// Preserved ARD trust metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct TrustManifest(Value);

impl TrustManifest {
    pub fn new(value: Value) -> Result<Self, TrustManifestError> {
        let object = value.as_object().ok_or(TrustManifestError::NotObject)?;
        match object.get("identity") {
            Some(Value::String(identity)) if !identity.trim().is_empty() => {}
            Some(Value::String(_)) | None => return Err(TrustManifestError::MissingIdentity),
            Some(_) => return Err(TrustManifestError::InvalidIdentity),
        }
        if let Some(identity_type) = object.get("identityType") {
            match identity_type.as_str() {
                Some("spiffe" | "did" | "https" | "other") => {}
                _ => return Err(TrustManifestError::InvalidIdentityType),
            }
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
