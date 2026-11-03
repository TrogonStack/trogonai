use std::fmt;
use std::str::FromStr;

use sqlx::postgres::PgSslMode;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PostgresTls {
    #[default]
    VerifyFull,
    Disabled,
}

#[derive(Debug, thiserror::Error)]
pub enum PostgresTlsError {
    #[error(
        "sslmode={0} does not fully verify the server; use sslmode=verify-full, or disable TLS explicitly with sslmode=disable"
    )]
    Unverified(UnverifiedSslMode),
    #[error("sslmode is not a known mode; use sslmode=verify-full, or disable TLS explicitly with sslmode=disable")]
    Unknown(#[source] sqlx::Error),
}

impl TryFrom<PgSslMode> for PostgresTls {
    type Error = PostgresTlsError;

    fn try_from(mode: PgSslMode) -> Result<Self, Self::Error> {
        match mode {
            PgSslMode::VerifyFull => Ok(Self::VerifyFull),
            PgSslMode::Disable => Ok(Self::Disabled),
            PgSslMode::Allow => Err(PostgresTlsError::Unverified(UnverifiedSslMode::Allow)),
            PgSslMode::Prefer => Err(PostgresTlsError::Unverified(UnverifiedSslMode::Prefer)),
            PgSslMode::Require => Err(PostgresTlsError::Unverified(UnverifiedSslMode::Require)),
            PgSslMode::VerifyCa => Err(PostgresTlsError::Unverified(UnverifiedSslMode::VerifyCa)),
        }
    }
}

impl FromStr for PostgresTls {
    type Err = PostgresTlsError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        raw.parse::<PgSslMode>().map_err(PostgresTlsError::Unknown)?.try_into()
    }
}

impl From<PostgresTls> for PgSslMode {
    fn from(tls: PostgresTls) -> Self {
        match tls {
            PostgresTls::VerifyFull => Self::VerifyFull,
            PostgresTls::Disabled => Self::Disable,
        }
    }
}

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

#[cfg(test)]
mod tests;
