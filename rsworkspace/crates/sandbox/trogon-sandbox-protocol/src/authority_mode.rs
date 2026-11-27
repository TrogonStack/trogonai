use std::str::FromStr;

use trogon_std::env::ReadEnv;

use crate::AuthorityModeError;
use crate::constants::ENV_WORKER_AUTHORITY;

/// How far the authority guard goes when the broker does not list a held lease as the worker's.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum AuthorityMode {
    /// No checks.
    Off,
    /// Check, log and count; never freeze or purge.
    Warn,
    /// Check, freeze and purge.
    Enforce,
}

impl AuthorityMode {
    /// Parses `off`, `warn`, or `enforce`, ignoring case and surrounding space. An empty value means `warn`.
    pub fn parse(value: &str) -> Result<Self, AuthorityModeError> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" => Ok(Self::Off),
            "warn" | "" => Ok(Self::Warn),
            "enforce" => Ok(Self::Enforce),
            other => Err(AuthorityModeError {
                found: other.to_owned(),
            }),
        }
    }

    /// The mode for an unset or set `SANDBOX_WORKER_AUTHORITY` value; unset means `warn`.
    pub fn from_env_value(value: Option<&str>) -> Result<Self, AuthorityModeError> {
        value.map_or(Ok(Self::Warn), Self::parse)
    }

    /// The mode in `SANDBOX_WORKER_AUTHORITY`; unset means `warn`.
    pub fn from_env(env: &impl ReadEnv) -> Result<Self, AuthorityModeError> {
        Self::from_env_value(env.var(ENV_WORKER_AUTHORITY).ok().as_deref())
    }
}

impl FromStr for AuthorityMode {
    type Err = AuthorityModeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}
