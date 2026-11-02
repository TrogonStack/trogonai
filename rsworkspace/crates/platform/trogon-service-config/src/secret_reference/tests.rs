#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
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

#[cfg(unix)]
#[test]
fn rejects_a_value_that_is_not_unicode() {
    assert_eq!(
        SecretReference::try_from(OsStr::from_bytes(b"env://\xFF")),
        Err(SecretReferenceError::NotUnicode)
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

    assert_eq!(err.kind(), ErrorKind::ValueValidation);
    assert_eq!(
        err.get(ContextKind::InvalidArg),
        Some(&ContextValue::String("--token <TOKEN>".to_owned()))
    );
    assert_eq!(err.get(ContextKind::InvalidValue), None);
}

#[test]
fn parser_without_an_argument_still_redacts_the_rejected_value() {
    let error = SecretReferenceParser
        .parse_ref(&clap::Command::new("cli"), None, OsStr::new("literal-value"))
        .expect_err("a literal is not a secret reference");

    assert_eq!(error.kind(), ErrorKind::ValueValidation);
    assert_eq!(error.get(ContextKind::InvalidValue), None);
}

#[cfg(unix)]
#[test]
fn flag_rejects_a_value_that_is_not_unicode() {
    let err = ReferenceCli::try_parse_from([
        OsStr::new("cli"),
        OsStr::new("--token"),
        OsStr::from_bytes(b"env://\xFF"),
    ])
    .err()
    .unwrap();

    assert_eq!(err.kind(), ErrorKind::ValueValidation);
}
