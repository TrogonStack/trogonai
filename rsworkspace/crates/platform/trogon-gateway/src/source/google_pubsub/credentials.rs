use std::fmt;

use google_cloud_auth::credentials::{Credentials, external_account, service_account};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoogleCredentialsKind {
    ServiceAccount,
    ExternalAccount,
}

impl GoogleCredentialsKind {
    fn from_type_field(value: &str) -> Option<Self> {
        match value {
            "service_account" => Some(Self::ServiceAccount),
            "external_account" => Some(Self::ExternalAccount),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::ServiceAccount => "service_account",
            Self::ExternalAccount => "external_account",
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GoogleCredentialsError {
    #[error("must not be empty")]
    Empty,
    #[error("must be a valid JSON document")]
    InvalidJson,
    #[error("must be a JSON object")]
    NotAnObject,
    #[error("must declare a string \"type\" field")]
    MissingType,
    #[error("unsupported credentials type, supported types are service_account and external_account")]
    UnsupportedType,
    #[error("service_account credentials are malformed")]
    MalformedServiceAccount,
    #[error("external_account credentials are malformed")]
    MalformedExternalAccount,
}

#[derive(Clone)]
pub struct GoogleCredentials {
    kind: GoogleCredentialsKind,
    document: Value,
}

impl GoogleCredentials {
    pub fn parse(document: impl AsRef<str>) -> Result<Self, GoogleCredentialsError> {
        let document = document.as_ref().trim();
        if document.is_empty() {
            return Err(GoogleCredentialsError::Empty);
        }
        let value: Value = serde_json::from_str(document).map_err(|_| GoogleCredentialsError::InvalidJson)?;
        let Some(object) = value.as_object() else {
            return Err(GoogleCredentialsError::NotAnObject);
        };
        let declared_type = object
            .get("type")
            .and_then(Value::as_str)
            .ok_or(GoogleCredentialsError::MissingType)?;
        let kind =
            GoogleCredentialsKind::from_type_field(declared_type).ok_or(GoogleCredentialsError::UnsupportedType)?;

        Ok(Self { kind, document: value })
    }

    #[cfg(test)]
    pub fn kind(&self) -> GoogleCredentialsKind {
        self.kind
    }

    /// Must run inside a Tokio runtime because the official builders start a background token refresh task.
    pub fn build(&self) -> Result<Credentials, GoogleCredentialsError> {
        let document = self.document.clone();
        match self.kind {
            GoogleCredentialsKind::ServiceAccount => service_account::Builder::new(document)
                .build()
                .map_err(|_| GoogleCredentialsError::MalformedServiceAccount),
            GoogleCredentialsKind::ExternalAccount => external_account::Builder::new(document)
                .build()
                .map_err(|_| GoogleCredentialsError::MalformedExternalAccount),
        }
    }
}

impl fmt::Debug for GoogleCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GoogleCredentials({}, ****)", self.kind.label())
    }
}

#[cfg(test)]
pub(crate) mod fixtures;

#[cfg(test)]
mod tests;
