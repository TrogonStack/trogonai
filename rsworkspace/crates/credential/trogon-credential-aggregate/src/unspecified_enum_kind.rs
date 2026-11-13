use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnspecifiedEnumKind {
    SourceKind,
    CredentialKind,
    CredentialIdentityVersion,
}

impl fmt::Display for UnspecifiedEnumKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::SourceKind => "source kind",
            Self::CredentialKind => "credential kind",
            Self::CredentialIdentityVersion => "credential identity version",
        })
    }
}
