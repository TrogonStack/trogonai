use std::fmt;

use super::constants::{PRODUCTION_API_BASE_URL, SANDBOX_API_BASE_URL};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown environment {0:?}, expected sandbox or production")]
pub struct PlaidEnvironmentError(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaidEnvironment {
    Sandbox,
    Production,
}

impl PlaidEnvironment {
    pub fn parse(value: &str) -> Result<Self, PlaidEnvironmentError> {
        match value {
            "sandbox" => Ok(Self::Sandbox),
            "production" => Ok(Self::Production),
            other => Err(PlaidEnvironmentError(other.to_owned())),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sandbox => "sandbox",
            Self::Production => "production",
        }
    }

    pub fn default_api_base_url(self) -> &'static str {
        match self {
            Self::Sandbox => SANDBOX_API_BASE_URL,
            Self::Production => PRODUCTION_API_BASE_URL,
        }
    }
}

impl fmt::Display for PlaidEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests;
