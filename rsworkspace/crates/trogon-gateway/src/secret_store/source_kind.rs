use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SourceKind {
    Discord,
    GitHub,
    Gitlab,
    Incidentio,
    Linear,
    MicrosoftGraph,
    Notion,
    Sentry,
    Slack,
    Telegram,
    Twitter,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discord => "discord",
            Self::GitHub => "github",
            Self::Gitlab => "gitlab",
            Self::Incidentio => "incidentio",
            Self::Linear => "linear",
            Self::MicrosoftGraph => "microsoft_graph",
            Self::Notion => "notion",
            Self::Sentry => "sentry",
            Self::Slack => "slack",
            Self::Telegram => "telegram",
            Self::Twitter => "twitter",
        }
    }
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
