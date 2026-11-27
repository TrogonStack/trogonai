use std::fmt;

use serde::Deserialize;
use trogon_sandbox_contract::SandboxId;

use crate::SpaceIdError;

/// An identifier SPACE assigned, checked to be safe to place in a URL path.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Deserialize)]
#[serde(try_from = "String")]
pub struct SpaceId(SandboxId);

impl SpaceId {
    /// The longest identifier accepted.
    pub const MAX_LEN: usize = 128;

    /// Parses an identifier made of ASCII letters, digits, `_` and `-`.
    pub fn parse(value: impl Into<String>) -> Result<Self, SpaceIdError> {
        let id = SandboxId::new(value)?;
        if id.as_str().len() > Self::MAX_LEN {
            return Err(SpaceIdError::TooLong { max: Self::MAX_LEN });
        }
        if !id
            .as_str()
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err(SpaceIdError::Character);
        }
        Ok(Self(id))
    }

    /// The identifier as text.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    /// The identifier as the sandbox identifier the lease protocol carries.
    pub fn sandbox_id(&self) -> &SandboxId {
        &self.0
    }
}

impl TryFrom<String> for SpaceId {
    type Error = SpaceIdError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl fmt::Display for SpaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
