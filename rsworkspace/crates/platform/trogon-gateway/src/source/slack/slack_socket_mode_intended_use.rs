use std::fmt;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlackSocketModeIntendedUse {
    Development,
    BehindFirewall,
}

impl SlackSocketModeIntendedUse {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::BehindFirewall => "behind_firewall",
        }
    }
}

impl fmt::Display for SlackSocketModeIntendedUse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SlackSocketModeIntendedUse {
    type Err = SlackSocketModeIntendedUseError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "development" => Ok(Self::Development),
            "behind_firewall" => Ok(Self::BehindFirewall),
            _ => Err(SlackSocketModeIntendedUseError::Unsupported(value.to_string())),
        }
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum SlackSocketModeIntendedUseError {
    #[error(
        "required because Slack does not intend Socket Mode for widely distributed apps; use HTTP webhooks for production, or set 'development' or 'behind_firewall'"
    )]
    Missing,
    #[error(
        "unsupported value '{0}'; Slack does not intend Socket Mode for widely distributed apps, expected 'development' or 'behind_firewall'"
    )]
    Unsupported(String),
}

#[cfg(test)]
mod tests;
