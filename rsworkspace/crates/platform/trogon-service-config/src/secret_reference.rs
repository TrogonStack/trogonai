use std::ffi::OsStr;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use clap::builder::{TypedValueParser, ValueParserFactory};
use clap::error::{ContextKind, ContextValue, ErrorKind};
use trogon_nats::CredentialValue;

use crate::env_var_name::{EnvVarName, EnvVarNameError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SecretReference {
    Env(EnvVarName),
    File(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SecretReferenceError {
    #[error("must be a reference, env://NAME or file:///path, never the secret itself")]
    NotAReference,
    #[error("scheme `{0}` is not supported; use env://NAME or file:///path")]
    UnsupportedScheme(String),
    #[error("env://NAME reference is invalid: {0}")]
    InvalidEnvVarName(#[from] EnvVarNameError),
    #[error("file:///path reference names no path")]
    EmptyFilePath,
    #[error("must be a reference, env://NAME or file:///path, written as UTF-8")]
    NotUnicode,
}

impl FromStr for SecretReference {
    type Err = SecretReferenceError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        match CredentialValue::parse(raw) {
            Some(CredentialValue::Env(name)) => Ok(Self::Env(EnvVarName::new(name)?)),
            Some(CredentialValue::File(path)) if path.as_os_str().is_empty() => {
                Err(SecretReferenceError::EmptyFilePath)
            }
            Some(CredentialValue::File(path)) => Ok(Self::File(path)),
            Some(CredentialValue::Unsupported(scheme)) => Err(SecretReferenceError::UnsupportedScheme(scheme)),
            Some(CredentialValue::Literal(_)) | None => Err(SecretReferenceError::NotAReference),
        }
    }
}

impl TryFrom<&OsStr> for SecretReference {
    type Error = SecretReferenceError;

    fn try_from(raw: &OsStr) -> Result<Self, Self::Error> {
        raw.to_str().ok_or(SecretReferenceError::NotUnicode)?.parse()
    }
}

impl fmt::Display for SecretReference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(name) => write!(f, "env://{name}"),
            Self::File(path) => write!(f, "file://{}", path.display()),
        }
    }
}

/// Clap's default parse error quotes the rejected value, which here may be the
/// secret itself, so this parser reports why a value was rejected without it.
#[derive(Clone, Copy, Debug, Default)]
pub struct SecretReferenceParser;

impl TypedValueParser for SecretReferenceParser {
    type Value = SecretReference;

    fn parse_ref(
        &self,
        cmd: &clap::Command,
        arg: Option<&clap::Arg>,
        value: &OsStr,
    ) -> Result<Self::Value, clap::Error> {
        SecretReference::try_from(value).map_err(|error| {
            let message = match arg {
                Some(arg) => format!("invalid value for '{arg}': {error}"),
                None => error.to_string(),
            };
            let mut rejection = clap::Error::raw(ErrorKind::ValueValidation, message).format(&mut cmd.clone());
            if let Some(arg) = arg {
                rejection.insert(ContextKind::InvalidArg, ContextValue::String(arg.to_string()));
            }
            rejection
        })
    }
}

impl ValueParserFactory for SecretReference {
    type Parser = SecretReferenceParser;

    fn value_parser() -> Self::Parser {
        SecretReferenceParser
    }
}

#[cfg(test)]
mod tests;
