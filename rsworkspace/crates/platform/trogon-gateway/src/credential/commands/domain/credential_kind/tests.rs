use super::*;

macro_rules! every_kind {
    ($($variant:ident),+ $(,)?) => {{
        let kinds = [$(CredentialKind::$variant),+];
        for kind in kinds {
            // No wildcard arm: a variant missing from the list fails to compile.
            match kind {
                $(CredentialKind::$variant => {})+
            }
        }
        kinds
    }};
}

#[test]
fn every_kind_round_trips_through_as_str_and_parse() {
    let kinds = every_kind![
        AppToken,
        BotToken,
        ClientSecret,
        ClientState,
        ConsumerSecret,
        SecretToken,
        ServiceAccountCredentials,
        SigningSecret,
        SigningToken,
        VerificationToken,
        WebhookSecret,
        WebhookToken,
    ];

    for kind in kinds {
        assert_eq!(CredentialKind::parse(kind.as_str()), Some(kind));
    }
}
