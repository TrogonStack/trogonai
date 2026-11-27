use serde::{Deserialize, Serialize};

use crate::{EnvVarName, ExtractPattern, JsonKeyPath, JwtMasking};

/// How the credential is located inside a file.
///
/// `Regex` replaces capture group 1 of every match, `JsonKey` the string value at a key path, and `EnvLine` the value
/// of a `NAME=value` or `export NAME=value` line, quotes kept. `Jwt` replaces every JSON web token found in the file.
/// An environment variable target takes only `Regex` or `Jwt`. It serializes as an object with one key, the
/// snake-case variant name.
#[derive(Debug, Clone, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Extract {
    Regex(ExtractPattern),
    JsonKey(JsonKeyPath),
    EnvLine(EnvVarName),
    Jwt(JwtMasking),
}

impl Extract {
    /// Reports whether an environment variable target accepts this rule.
    pub const fn fits_env_target(&self) -> bool {
        matches!(self, Self::Regex(_) | Self::Jwt(_))
    }
}
