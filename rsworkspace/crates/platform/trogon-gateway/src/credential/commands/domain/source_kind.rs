use std::fmt;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SourceKind {
    Datadog,
    Discord,
    GitHub,
    Gitlab,
    GooglePubSub,
    Incidentio,
    Linear,
    MicrosoftGraph,
    Notion,
    Sentry,
    Slack,
    Telegram,
    Twitter,
    Zoom,
}

impl SourceKind {
    const ALL: [Self; 14] = [
        Self::Datadog,
        Self::Discord,
        Self::GitHub,
        Self::Gitlab,
        Self::GooglePubSub,
        Self::Incidentio,
        Self::Linear,
        Self::MicrosoftGraph,
        Self::Notion,
        Self::Sentry,
        Self::Slack,
        Self::Telegram,
        Self::Twitter,
        Self::Zoom,
    ];

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|source| source.as_str() == value)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Datadog => "datadog",
            Self::Discord => "discord",
            Self::GitHub => "github",
            Self::Gitlab => "gitlab",
            Self::GooglePubSub => "google_pubsub",
            Self::Incidentio => "incidentio",
            Self::Linear => "linear",
            Self::MicrosoftGraph => "microsoft_graph",
            Self::Notion => "notion",
            Self::Sentry => "sentry",
            Self::Slack => "slack",
            Self::Telegram => "telegram",
            Self::Twitter => "twitter",
            Self::Zoom => "zoom",
        }
    }
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! every_source_kind {
        ($($variant:ident),+ $(,)?) => {{
            let sources = [$(SourceKind::$variant),+];
            for source in sources {
                // No wildcard arm: a variant missing from the list fails to compile.
                match source {
                    $(SourceKind::$variant => {})+
                }
            }
            sources
        }};
    }

    #[test]
    fn every_source_kind_is_in_all_and_round_trips_through_as_str_and_parse() {
        let sources = every_source_kind![
            Datadog,
            Discord,
            GitHub,
            Gitlab,
            GooglePubSub,
            Incidentio,
            Linear,
            MicrosoftGraph,
            Notion,
            Sentry,
            Slack,
            Telegram,
            Twitter,
            Zoom,
        ];

        for source in sources {
            assert!(
                SourceKind::ALL.contains(&source),
                "{source} is missing from SourceKind::ALL"
            );
            assert_eq!(SourceKind::parse(source.as_str()), Some(source));
        }
        assert_eq!(SourceKind::ALL.len(), sources.len());
    }
}
