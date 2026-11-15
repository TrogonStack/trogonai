use std::fmt;
use std::sync::Arc;

use super::OpenBaoNameError;

/// The name of an OpenBao ACL policy, as a token request names it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenBaoPolicyName(Arc<str>);

impl OpenBaoPolicyName {
    pub fn new(value: impl AsRef<str>) -> Result<Self, OpenBaoNameError> {
        let value = value.as_ref();
        OpenBaoNameError::check(value)?;
        Ok(Self(Arc::from(value)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OpenBaoPolicyName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
