use std::fmt;
use std::sync::Arc;

use super::OpenBaoNameError;

/// The OpenBao token role every per-request child token is minted through.
///
/// The role fixes which policies a child may carry and keeps every child
/// attached to the login token, so the login token's own policy needs to grant
/// nothing but creating tokens through this role.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenBaoTokenRole(Arc<str>);

impl OpenBaoTokenRole {
    pub fn new(value: impl AsRef<str>) -> Result<Self, OpenBaoNameError> {
        let value = value.as_ref();
        OpenBaoNameError::check(value)?;
        Ok(Self(Arc::from(value)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OpenBaoTokenRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
