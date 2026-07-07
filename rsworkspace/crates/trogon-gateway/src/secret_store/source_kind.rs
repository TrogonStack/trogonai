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
    const ALL: [Self; 11] = [
        Self::Discord,
        Self::GitHub,
        Self::Gitlab,
        Self::Incidentio,
        Self::Linear,
        Self::MicrosoftGraph,
        Self::Notion,
        Self::Sentry,
        Self::Slack,
        Self::Telegram,
        Self::Twitter,
    ];

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|source| source.as_str() == value)
    }

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
