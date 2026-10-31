use super::*;

#[test]
fn defaults_to_verifying_the_server() {
    assert_eq!(PostgresTls::default(), PostgresTls::VerifyFull);
}

#[test]
fn accepts_verify_full_and_an_explicit_disable() {
    assert_eq!("verify-full".parse::<PostgresTls>().unwrap(), PostgresTls::VerifyFull);
    assert_eq!("disable".parse::<PostgresTls>().unwrap(), PostgresTls::Disabled);
}

#[test]
fn accepts_the_mode_in_any_case() {
    assert_eq!("VERIFY-FULL".parse::<PostgresTls>().unwrap(), PostgresTls::VerifyFull);
    assert_eq!("Disable".parse::<PostgresTls>().unwrap(), PostgresTls::Disabled);
}

#[test]
fn rejects_modes_that_do_not_fully_verify_the_server() {
    for mode in ["allow", "prefer", "require", "verify-ca"] {
        let error = mode.parse::<PostgresTls>().unwrap_err();

        assert!(matches!(error, PostgresTlsError::Unverified(_)), "{mode}");
        assert!(error.to_string().starts_with(&format!("sslmode={mode} ")), "{error}");
    }
}

#[test]
fn rejects_modes_it_does_not_know() {
    for mode in ["verify_full", "on", ""] {
        assert!(
            matches!(mode.parse::<PostgresTls>(), Err(PostgresTlsError::Unknown(_))),
            "{mode:?}"
        );
    }
}

#[test]
fn rejection_offers_verification_or_disabling_tls_explicitly() {
    for error in [
        "require".parse::<PostgresTls>().unwrap_err(),
        "bogus".parse::<PostgresTls>().unwrap_err(),
    ] {
        let message = error.to_string();

        assert!(message.contains("use sslmode=verify-full"), "{message}");
        assert!(
            message.contains("disable TLS explicitly with sslmode=disable"),
            "{message}"
        );
    }
}

#[test]
fn converts_from_the_sqlx_mode() {
    assert_eq!(
        PostgresTls::try_from(PgSslMode::VerifyFull).unwrap(),
        PostgresTls::VerifyFull
    );
    assert_eq!(
        PostgresTls::try_from(PgSslMode::Disable).unwrap(),
        PostgresTls::Disabled
    );
    assert!(matches!(
        PostgresTls::try_from(PgSslMode::Prefer),
        Err(PostgresTlsError::Unverified(PgSslMode::Prefer))
    ));
}

#[test]
fn converts_into_the_sqlx_mode() {
    assert!(matches!(
        PgSslMode::from(PostgresTls::VerifyFull),
        PgSslMode::VerifyFull
    ));
    assert!(matches!(PgSslMode::from(PostgresTls::Disabled), PgSslMode::Disable));
}
