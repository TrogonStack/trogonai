use crate::Error;
use sqlx::{
    ConnectOptions,
    postgres::{PgConnectOptions, PgSslMode},
};
use std::str::FromStr;

pub struct DatabaseOptions(PgConnectOptions);
impl DatabaseOptions {
    pub fn new(dsn: &str) -> Result<Self, Error> {
        let options = PgConnectOptions::from_str(dsn).map_err(|_| Error::Configuration)?;
        let local = matches!(options.get_host(), "localhost" | "127.0.0.1" | "::1" | "[::1]");
        if !local && !matches!(options.get_ssl_mode(), PgSslMode::VerifyFull) {
            return Err(Error::Configuration);
        }
        Ok(Self(options.disable_statement_logging()))
    }
    pub(crate) fn into_inner(self) -> PgConnectOptions {
        self.0
    }
}
