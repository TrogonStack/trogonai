use std::fmt;
use std::path::{Path, PathBuf};

use sqlx::ConnectOptions;
use sqlx::postgres::PgConnectOptions;
use trogon_std::env::ReadEnv;
use url::Url;

use super::postgres_tls::{PostgresTls, PostgresTlsError};
use crate::constants::{
    ENV_PGSSLMODE, ENV_PGSSLROOTCERT, POSTGRES_URL_SCHEMES, SSL_MODE_QUERY_KEYS, SSL_ROOT_CERT_QUERY_KEYS,
};

#[derive(Clone)]
pub struct PostgresUrl {
    options: PgConnectOptions,
    tls: PostgresTls,
    root_cert: Option<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum PostgresUrlError {
    #[error("Postgres URL is malformed: {0}")]
    Malformed(#[source] url::ParseError),
    #[error("Postgres URL scheme `{0}` is not supported; use postgres:// or postgresql://")]
    UnsupportedScheme(String),
    #[error("Postgres URL: {0}")]
    SslMode(#[source] PostgresTlsError),
    #[error("PGSSLMODE: {0}")]
    EnvSslMode(#[source] PostgresTlsError),
    #[error("Postgres URL is not usable: {0}")]
    Unusable(#[source] sqlx::Error),
}

impl PostgresUrl {
    pub fn parse(raw: &str, env: &impl ReadEnv) -> Result<Self, PostgresUrlError> {
        let url = Url::parse(raw).map_err(PostgresUrlError::Malformed)?;
        if !POSTGRES_URL_SCHEMES.contains(&url.scheme()) {
            return Err(PostgresUrlError::UnsupportedScheme(url.scheme().to_owned()));
        }

        let tls = match last_query_value(&url, &SSL_MODE_QUERY_KEYS) {
            Some(mode) => mode.parse().map_err(PostgresUrlError::SslMode)?,
            None => match env.var(ENV_PGSSLMODE) {
                Ok(mode) if !mode.is_empty() => mode.parse().map_err(PostgresUrlError::EnvSslMode)?,
                _ => PostgresTls::default(),
            },
        };
        let root_cert = last_query_value(&url, &SSL_ROOT_CERT_QUERY_KEYS)
            .map(PathBuf::from)
            .or_else(|| {
                env.var_os(ENV_PGSSLROOTCERT)
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from)
            });

        let mut options = PgConnectOptions::from_url(&url)
            .map_err(PostgresUrlError::Unusable)?
            .ssl_mode(tls.into());
        if let Some(path) = &root_cert {
            options = options.ssl_root_cert(path);
        }

        Ok(Self {
            options,
            tls,
            root_cert,
        })
    }

    pub const fn tls(&self) -> PostgresTls {
        self.tls
    }

    pub fn root_cert(&self) -> Option<&Path> {
        self.root_cert.as_deref()
    }

    pub(crate) const fn connect_options(&self) -> &PgConnectOptions {
        &self.options
    }
}

impl fmt::Debug for PostgresUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostgresUrl")
            .field("host", &self.options.get_host())
            .field("port", &self.options.get_port())
            .field("database", &self.options.get_database())
            .field("tls", &self.tls)
            .field("root_cert", &self.root_cert)
            .finish_non_exhaustive()
    }
}

fn last_query_value(url: &Url, keys: &[&str]) -> Option<String> {
    url.query_pairs()
        .filter(|(key, _)| keys.contains(&key.as_ref()))
        .last()
        .map(|(_, value)| value.into_owned())
}

#[cfg(test)]
mod tests;
