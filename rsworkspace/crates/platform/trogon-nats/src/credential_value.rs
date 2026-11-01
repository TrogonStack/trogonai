use crate::constants::SEED_PREFIXES;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use trogon_std::env::ReadEnv;
use trogon_std::secret_string::SecretString;

#[derive(Debug, thiserror::Error)]
pub enum SettingError {
    #[error("scheme `{0}` is not supported")]
    UnsupportedScheme(String),
    #[error("cannot read {}: {source}", path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("environment variable `{0}` is not set")]
    EnvUnset(String),
    #[error("invalid NKey seed: {0}")]
    InvalidSeed(#[source] nkeys::error::Error),
    #[error("cannot read the NKey seed file: {0}")]
    SeedFileRead(#[source] io::Error),
    #[error("no NKey seed found in the NKey seed file")]
    NoSeedFound,
    #[error("must be set together with {0}")]
    MissingCompanion(&'static str),
    #[error("cannot be combined with {0}")]
    ConflictsWith(&'static str),
    #[error("`{value}` is not one of {}", accepted.join(", "))]
    UnrecognizedValue {
        value: String,
        accepted: &'static [&'static str],
    },
}

fn split_scheme(raw: &str) -> Option<(&str, &str)> {
    let end = raw.find(':')?;
    let scheme = &raw[..end];
    let mut chars = scheme.chars();
    let starts_alphabetic = chars.next().is_some_and(|c| c.is_ascii_alphabetic());
    let well_formed = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'));
    if scheme.len() < 2 || !starts_alphabetic || !well_formed {
        return None;
    }
    let rest = &raw[end + 1..];
    Some((scheme, rest.strip_prefix("//").unwrap_or(rest)))
}

/// A filesystem location written as a bare path or as `file://path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Location {
    Path(PathBuf),
    Unsupported(String),
}

impl Location {
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.is_empty() {
            return None;
        }
        Some(match split_scheme(raw) {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("file") => Self::Path(PathBuf::from(rest)),
            Some((scheme, _)) => Self::Unsupported(scheme.to_ascii_lowercase()),
            None => Self::Path(PathBuf::from(raw)),
        })
    }

    pub fn path(&self) -> Result<&Path, SettingError> {
        match self {
            Self::Path(path) => Ok(path),
            Self::Unsupported(scheme) => Err(SettingError::UnsupportedScheme(scheme.clone())),
        }
    }

    /// Reads an NKey seed from a plain or decorated seed file (including a
    /// `.creds` file), taking the first line that starts with a seed prefix.
    /// Errors never echo the path, because a mistyped inline seed is read as
    /// a path.
    pub async fn read_seed(&self) -> Result<String, SettingError> {
        let path = self.path()?;
        let contents = tokio::fs::read_to_string(path)
            .await
            .map_err(SettingError::SeedFileRead)?;
        let seed = contents
            .lines()
            .map(str::trim)
            .find(|line| SEED_PREFIXES.iter().any(|prefix| line.starts_with(prefix)))
            .ok_or(SettingError::NoSeedFound)?;
        nkeys::KeyPair::from_seed(seed).map_err(SettingError::InvalidSeed)?;
        Ok(seed.to_owned())
    }
}

/// An NKey seed given inline, as a seed file location, or as an `env://NAME`
/// reference to the variable holding it. A value that parses as a seed is used
/// inline; any other value that is not an `env://` reference is a bare or
/// `file://` path, which is how the `nats` CLI reads `NATS_NKEY`.
#[derive(Clone)]
pub enum NKeySeed {
    Inline(SecretString),
    File(Location),
    Env(String),
}

impl NKeySeed {
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if nkeys::KeyPair::from_seed(trimmed).is_ok() {
            return SecretString::new(trimmed).ok().map(Self::Inline);
        }
        match split_scheme(trimmed) {
            Some((scheme, name)) if scheme.eq_ignore_ascii_case("env") => Some(Self::Env(name.to_owned())),
            _ => Location::parse(trimmed).map(Self::File),
        }
    }

    pub async fn resolve<E: ReadEnv>(&self, env: &E) -> Result<String, SettingError> {
        match self {
            Self::Inline(seed) => Ok(seed.as_str().to_owned()),
            Self::File(location) => location.read_seed().await,
            Self::Env(name) => {
                let value = env.var(name).map_err(|_| SettingError::EnvUnset(name.clone()))?;
                let seed = value.trim();
                nkeys::KeyPair::from_seed(seed).map_err(SettingError::InvalidSeed)?;
                Ok(seed.to_owned())
            }
        }
    }
}

impl fmt::Debug for NKeySeed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inline(_) => f.debug_tuple("Inline").field(&"<redacted>").finish(),
            Self::File(_) => f.debug_tuple("File").field(&"<redacted>").finish(),
            Self::Env(name) => f.debug_tuple("Env").field(name).finish(),
        }
    }
}

/// A configured value that is either a literal or a reference to where the
/// value lives (`file://path`, `env://NAME`).
#[derive(Clone)]
pub enum CredentialValue {
    Literal(SecretString),
    File(PathBuf),
    Env(String),
    Unsupported(String),
}

impl CredentialValue {
    pub fn parse(raw: &str) -> Option<Self> {
        match split_scheme(raw) {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("file") => Some(Self::File(PathBuf::from(rest))),
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("env") => Some(Self::Env(rest.to_owned())),
            Some((scheme, _)) if ["op", "nsc", "data"].iter().any(|s| scheme.eq_ignore_ascii_case(s)) => {
                Some(Self::Unsupported(scheme.to_ascii_lowercase()))
            }
            _ => SecretString::new(raw).ok().map(Self::Literal),
        }
    }

    pub async fn resolve<E: ReadEnv>(&self, env: &E) -> Result<String, SettingError> {
        match self {
            Self::Literal(value) => Ok(value.as_str().to_owned()),
            Self::File(path) => read_file(path)
                .await
                .map(|contents| contents.trim_end_matches(['\r', '\n']).to_owned()),
            Self::Env(name) => env.var(name).map_err(|_| SettingError::EnvUnset(name.clone())),
            Self::Unsupported(scheme) => Err(SettingError::UnsupportedScheme(scheme.clone())),
        }
    }
}

impl fmt::Debug for CredentialValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Literal(_) => f.debug_tuple("Literal").field(&"<redacted>").finish(),
            Self::File(path) => f.debug_tuple("File").field(path).finish(),
            Self::Env(name) => f.debug_tuple("Env").field(name).finish(),
            Self::Unsupported(scheme) => f.debug_tuple("Unsupported").field(scheme).finish(),
        }
    }
}

async fn read_file(path: &Path) -> Result<String, SettingError> {
    tokio::fs::read_to_string(path)
        .await
        .map_err(|source| SettingError::Read {
            path: path.to_path_buf(),
            source,
        })
}

#[cfg(test)]
mod tests;
