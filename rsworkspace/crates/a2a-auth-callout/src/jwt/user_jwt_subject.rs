use std::fmt;

use crate::wire::NkeyPublic;

/// User NKey public embedded as the NATS User JWT `sub` claim.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UserJwtSubject(NkeyPublic);

impl UserJwtSubject {
    pub fn from_user_nkey(nkey: NkeyPublic) -> Self {
        Self(nkey)
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn nkey(&self) -> &NkeyPublic {
        &self.0
    }
}

impl fmt::Display for UserJwtSubject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
