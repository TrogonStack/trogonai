use std::fmt;

/// Which extract rule a failure belongs to.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ExtractKind {
    Regex,
    JsonKey,
    EnvLine,
    Jwt,
}

impl fmt::Display for ExtractKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Regex => "regex",
            Self::JsonKey => "json_key",
            Self::EnvLine => "env_line",
            Self::Jwt => "jwt",
        })
    }
}
