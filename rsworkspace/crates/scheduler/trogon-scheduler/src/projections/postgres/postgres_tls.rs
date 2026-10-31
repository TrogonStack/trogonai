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
        "sslmode={} does not fully verify the server; use sslmode=verify-full, or disable TLS explicitly with sslmode=disable",
        libpq_name(.0)
    )]
    Unverified(PgSslMode),
    #[error("sslmode is not a known mode; use sslmode=verify-full, or disable TLS explicitly with sslmode=disable")]
    Unknown(#[source] sqlx::Error),
}

impl TryFrom<PgSslMode> for PostgresTls {
    type Error = PostgresTlsError;

    fn try_from(mode: PgSslMode) -> Result<Self, Self::Error> {
        match mode {
            PgSslMode::VerifyFull => Ok(Self::VerifyFull),
            PgSslMode::Disable => Ok(Self::Disabled),
            PgSslMode::Allow | PgSslMode::Prefer | PgSslMode::Require | PgSslMode::VerifyCa => {
                Err(PostgresTlsError::Unverified(mode))
            }
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

fn libpq_name(mode: &PgSslMode) -> &'static str {
    match mode {
        PgSslMode::Disable => "disable",
        PgSslMode::Allow => "allow",
        PgSslMode::Prefer => "prefer",
        PgSslMode::Require => "require",
        PgSslMode::VerifyCa => "verify-ca",
        PgSslMode::VerifyFull => "verify-full",
    }
}

#[cfg(test)]
mod tests;
