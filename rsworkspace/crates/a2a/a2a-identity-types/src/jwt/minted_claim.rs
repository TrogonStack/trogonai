use std::fmt;

/// A claim the auth callout writes into every user JWT it mints and a holder
/// of the token reads back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MintedClaim {
    /// The caller id the callout derived for the credential it verified.
    CallerId,
    /// The wire name of the kind of actor that credential names.
    PrincipalKind,
}

impl MintedClaim {
    /// Returns the claim's name in the token payload.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CallerId => "caller_id",
            Self::PrincipalKind => "principal_kind",
        }
    }
}

impl fmt::Display for MintedClaim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.as_str().fmt(f)
    }
}

#[cfg(test)]
mod tests;
