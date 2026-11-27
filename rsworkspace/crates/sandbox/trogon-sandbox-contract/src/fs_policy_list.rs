use std::fmt;

/// One of the six lists of an [`FsPolicy`](crate::FsPolicy), named as on the wire.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Hash)]
pub enum FsPolicyList {
    DenyRead,
    CredentialDenyRead,
    AllowReadWithinDeny,
    OwnAllowWithinDeny,
    AllowWrite,
    DenyWrite,
}

impl fmt::Display for FsPolicyList {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::DenyRead => "deny_read",
            Self::CredentialDenyRead => "credential_deny_read",
            Self::AllowReadWithinDeny => "allow_read_within_deny",
            Self::OwnAllowWithinDeny => "own_allow_within_deny",
            Self::AllowWrite => "allow_write",
            Self::DenyWrite => "deny_write",
        })
    }
}
