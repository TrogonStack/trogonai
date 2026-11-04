use std::fmt;

/// A libpq `sslmode` that connects without fully verifying the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnverifiedSslMode {
    Allow,
    Prefer,
    Require,
    VerifyCa,
}

impl fmt::Display for UnverifiedSslMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Allow => "allow",
            Self::Prefer => "prefer",
            Self::Require => "require",
            Self::VerifyCa => "verify-ca",
        })
    }
}
