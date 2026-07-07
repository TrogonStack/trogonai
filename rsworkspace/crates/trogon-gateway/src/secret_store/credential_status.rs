use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialStatus {
    Pending,
    Active,
    Previous,
    Revoked,
    Expired,
}

impl CredentialStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::Previous => "previous",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
        }
    }

    pub fn is_readable(self) -> bool {
        matches!(self, Self::Active | Self::Previous)
    }
}

impl fmt::Display for CredentialStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
