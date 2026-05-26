use std::fmt;
use std::time::Duration;

use trogon_std::env::ReadEnv;

pub const ENV_SPICEDB_ENDPOINT: &str = "A2A_SPICEDB_ENDPOINT";
pub const ENV_SPICEDB_TOKEN: &str = "A2A_SPICEDB_TOKEN";
pub const ENV_SPICEDB_ZEDTOKEN_TTL_SECS: &str = "A2A_SPICEDB_ZEDTOKEN_TTL_SECS";

const DEFAULT_ZEDTOKEN_TTL_SECS: u64 = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpiceDbEndpoint(String);

impl SpiceDbEndpoint {
    pub fn parse(raw: impl Into<String>) -> Result<Self, SpiceDbImportGateBuildError> {
        let raw = raw.into();
        if raw.trim().is_empty() {
            return Err(SpiceDbImportGateBuildError::InvalidEndpoint(
                "endpoint must not be empty".into(),
            ));
        }
        Ok(Self(raw))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct SpiceDbToken(String);

impl SpiceDbToken {
    pub fn parse(raw: impl Into<String>) -> Result<Self, SpiceDbImportGateBuildError> {
        let raw = raw.into();
        if raw.trim().is_empty() {
            return Err(SpiceDbImportGateBuildError::InvalidToken(
                "token must not be empty".into(),
            ));
        }
        Ok(Self(raw))
    }

    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SpiceDbToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SpiceDbToken(***)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZedTokenTtl(Duration);

impl ZedTokenTtl {
    pub fn from_secs(secs: u64) -> Self {
        Self(Duration::from_secs(secs.max(1)))
    }

    pub fn as_duration(self) -> Duration {
        self.0
    }
}

impl Default for ZedTokenTtl {
    fn default() -> Self {
        Self(Duration::from_secs(DEFAULT_ZEDTOKEN_TTL_SECS))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpiceDbImportGateBuildError {
    InvalidEndpoint(String),
    InvalidToken(String),
    InvalidZedTokenTtl(String),
    Connect(String),
}

impl fmt::Display for SpiceDbImportGateBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidEndpoint(msg) => write!(f, "invalid SpiceDB endpoint: {msg}"),
            Self::InvalidToken(msg) => write!(f, "invalid SpiceDB token: {msg}"),
            Self::InvalidZedTokenTtl(msg) => write!(f, "invalid SpiceDB ZedToken TTL: {msg}"),
            Self::Connect(msg) => write!(f, "SpiceDB connect failed: {msg}"),
        }
    }
}

impl std::error::Error for SpiceDbImportGateBuildError {}

pub fn zed_token_ttl_from_env<E: ReadEnv>(env: &E) -> Result<ZedTokenTtl, SpiceDbImportGateBuildError> {
    match env.var(ENV_SPICEDB_ZEDTOKEN_TTL_SECS) {
        Ok(raw) => raw
            .parse::<u64>()
            .map(ZedTokenTtl::from_secs)
            .map_err(|_| SpiceDbImportGateBuildError::InvalidZedTokenTtl(raw)),
        Err(std::env::VarError::NotPresent) => Ok(ZedTokenTtl::default()),
        Err(std::env::VarError::NotUnicode(_)) => Err(SpiceDbImportGateBuildError::InvalidZedTokenTtl(
            ENV_SPICEDB_ZEDTOKEN_TTL_SECS.into(),
        )),
    }
}

pub fn optional_spicedb_credentials<E: ReadEnv>(
    env: &E,
) -> Result<Option<(SpiceDbEndpoint, SpiceDbToken)>, SpiceDbImportGateBuildError> {
    let endpoint = match env.var(ENV_SPICEDB_ENDPOINT) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(SpiceDbImportGateBuildError::InvalidEndpoint(
                ENV_SPICEDB_ENDPOINT.into(),
            ));
        }
    };
    let token = match env.var(ENV_SPICEDB_TOKEN) {
        Ok(value) => Some(value),
        Err(std::env::VarError::NotPresent) => None,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err(SpiceDbImportGateBuildError::InvalidToken(ENV_SPICEDB_TOKEN.into()));
        }
    };

    match (endpoint, token) {
        (None, None) => Ok(None),
        (Some(endpoint), Some(token)) => Ok(Some((SpiceDbEndpoint::parse(endpoint)?, SpiceDbToken::parse(token)?))),
        (Some(_), None) => Err(SpiceDbImportGateBuildError::InvalidToken(format!(
            "{ENV_SPICEDB_TOKEN} is required when {ENV_SPICEDB_ENDPOINT} is set"
        ))),
        (None, Some(_)) => Err(SpiceDbImportGateBuildError::InvalidEndpoint(format!(
            "{ENV_SPICEDB_ENDPOINT} is required when {ENV_SPICEDB_TOKEN} is set"
        ))),
    }
}
