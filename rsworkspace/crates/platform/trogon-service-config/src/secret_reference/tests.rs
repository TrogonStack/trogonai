use std::path::Path;

use clap::Parser;

use super::*;

fn env(name: &str) -> SecretReference {
    SecretReference::Env(EnvVarName::new(name).unwrap())
}

#[test]
fn accepts_an_env_reference() {
    assert_eq!("env://NATS_TOKEN".parse(), Ok(env("NATS_TOKEN")));
}

#[test]
fn accepts_a_file_reference() {
    assert_eq!(
        "file:///run/secrets/nats-token".parse(),
        Ok(SecretReference::File("/run/secrets/nats-token".into()))
    );
}

#[test]
fn accepts_the_scheme_in_any_case() {
    assert_eq!("ENV://NATS_TOKEN".parse(), Ok(env("NATS_TOKEN")));
    assert!(matches!(
        "File:///run/token".parse(),
        Ok(SecretReference::File(ref path)) if path == Path::new("/run/token")
    ));
}

#[test]
fn rejects_literal_values() {
    for raw in [
        "hunter2",
        "eyJ0eXAiOiJKV1QiLCJhbGciOiJlZDI1NTE5LW5rZXkifQ.eyJqdGkiOiJYIn0.c2ln",
        "/run/secrets/nats-token",
        "pass:word",
        "",
    ] {
        assert_eq!(
            raw.parse::<SecretReference>(),
            Err(SecretReferenceError::NotAReference),
            "{raw}"
        );
    }
}

#[test]
fn rejects_schemes_that_are_not_resolved() {
    assert_eq!(
        "op://vault/item/field".parse::<SecretReference>(),
        Err(SecretReferenceError::UnsupportedScheme("op".to_owned()))
    );
}

#[test]
fn rejects_references_that_name_nothing() {
    assert_eq!(
        "env://".parse::<SecretReference>(),
        Err(SecretReferenceError::InvalidEnvVarName(EnvVarNameError::Empty))
    );
    assert_eq!(
        "env://A=B".parse::<SecretReference>(),
        Err(SecretReferenceError::InvalidEnvVarName(
            EnvVarNameError::ReservedCharacter
        ))
    );
    assert_eq!(
        "file://".parse::<SecretReference>(),
        Err(SecretReferenceError::EmptyFilePath)
    );
}

#[test]
fn displays_in_the_syntax_it_parses() {
    for raw in [
        "env://NATS_TOKEN",
        "file:///run/secrets/nats-token",
        "file://relative/token",
    ] {
        let reference: SecretReference = raw.parse().unwrap();

        assert_eq!(reference.to_string(), raw);
    }
}

#[test]
fn rejection_names_both_accepted_forms() {
    let message = SecretReferenceError::NotAReference.to_string();

    assert!(message.contains("env://NAME"));
    assert!(message.contains("file:///path"));
}

#[derive(Parser)]
struct ReferenceCli {
    #[arg(long)]
    token: Option<SecretReference>,
}

#[test]
fn flag_parses_a_reference() {
    let cli = ReferenceCli::try_parse_from(["cli", "--token", "env://NATS_TOKEN"]).unwrap();

    assert_eq!(cli.token, Some(env("NATS_TOKEN")));
}

#[test]
fn flag_rejection_never_repeats_the_value() {
    let err = ReferenceCli::try_parse_from(["cli", "--token", "hunter2"])
        .err()
        .unwrap();
    let message = err.to_string();

    assert_eq!(err.kind(), ErrorKind::ValueValidation);
    assert!(!message.contains("hunter2"), "{message}");
    assert!(message.contains("--token"), "{message}");
    assert!(message.contains("env://NAME"), "{message}");
    assert!(message.contains("file:///path"), "{message}");
}

#[cfg(unix)]
#[test]
fn flag_rejects_a_value_that_is_not_unicode() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let err = ReferenceCli::try_parse_from([
        OsString::from("cli"),
        OsString::from("--token"),
        OsString::from_vec(b"env://\xFF".to_vec()),
    ])
    .err()
    .unwrap();

    assert_eq!(err.kind(), ErrorKind::ValueValidation);
    assert!(err.to_string().contains("UTF-8"));
}
