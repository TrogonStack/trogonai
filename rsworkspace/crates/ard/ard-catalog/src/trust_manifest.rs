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
    #[error("trustManifest has unsupported field: {0}")]
    UnknownField(String),
    #[error("trustManifest.{field} must be {expected}")]
    InvalidFieldType {
        field: &'static str,
        expected: &'static str,
    },
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
        if let Some(trust_schema) = object.get("trustSchema")
            && !trust_schema.is_object()
        {
            return Err(TrustManifestError::InvalidFieldType {
                field: "trustSchema",
                expected: "an object",
            });
        }
        for field in ["attestations", "provenance"] {
            if let Some(value) = object.get(field)
                && !value.is_array()
            {
                return Err(TrustManifestError::InvalidFieldType {
                    field,
                    expected: "an array",
                });
            }
        }
        if let Some(signature) = object.get("signature")
            && !signature.is_string()
        {
            return Err(TrustManifestError::InvalidFieldType {
                field: "signature",
                expected: "a string",
            });
        }
        const ALLOWED_KEYS: &[&str] = &[
            "identity",
            "identityType",
            "trustSchema",
            "attestations",
            "provenance",
            "signature",
        ];
        for key in object.keys() {
            if !ALLOWED_KEYS.contains(&key.as_str()) {
                return Err(TrustManifestError::UnknownField(key.clone()));
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
