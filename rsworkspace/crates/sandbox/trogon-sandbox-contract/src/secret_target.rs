use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{EnvVarName, LiteralPath};

/// Where the sandboxed process sees the credential.
///
/// It is an environment variable or a file at an absolute path inside the sandbox. It serializes as an object with
/// one key, `env` or `file`.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretTarget {
    Env(EnvVarName),
    File(LiteralPath),
}

impl fmt::Display for SecretTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Env(name) => write!(formatter, "env '{name}'"),
            Self::File(path) => write!(formatter, "file '{path}'"),
        }
    }
}
