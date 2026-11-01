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
        Plaid,
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

#[test]
fn display_matches_as_str() {
    assert_eq!(SourceKind::GitHub.to_string(), SourceKind::GitHub.as_str());
}
