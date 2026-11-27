use std::fmt;

use secrecy::{ExposeSecret, SecretString};
use tonic::metadata::{Ascii, MetadataValue};

use crate::SpiceDbTokenError;

/// The preshared key that authenticates calls to the permission service.
///
/// Construction guarantees the token can be sent as a request header. The value is wrapped so it
/// never appears in `Debug` output or error messages.
pub struct SpiceDbToken(SecretString);

impl SpiceDbToken {
    pub fn new(value: impl Into<String>) -> Result<Self, SpiceDbTokenError> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(SpiceDbTokenError::Empty);
        }
        let token = Self(SecretString::from(value));
        token.authorization()?;
        Ok(token)
    }

    /// The `authorization` header value carrying this token, marked sensitive.
    pub(crate) fn authorization(&self) -> Result<MetadataValue<Ascii>, SpiceDbTokenError> {
        let mut value = MetadataValue::try_from(format!("Bearer {}", self.0.expose_secret()))
            .map_err(|_| SpiceDbTokenError::NotHeaderSafe)?;
        value.set_sensitive(true);
        Ok(value)
    }
}

impl fmt::Debug for SpiceDbToken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SpiceDbToken(***)")
    }
}
