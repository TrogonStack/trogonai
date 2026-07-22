use std::io::Write;

use tempfile::NamedTempFile;
use trogon_std::env::InMemoryEnv;

use super::signing_key_source_from_env;
use crate::error::AuthCalloutError;
use nkeys::KeyPair;

#[test]
fn env_source_loads_from_signing_secret() {
    let kp = KeyPair::new_account();
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_SIGNING_KEY_SOURCE", "env");
    env.set("AUTH_CALLOUT_SIGNING_SECRET", kp.seed().expect("seed"));

    let source = signing_key_source_from_env(&env).expect("env source");
    assert_eq!(source.accepted().len(), 1);
}

#[test]
fn env_source_falls_back_to_legacy_issuer_seed() {
    let kp = KeyPair::new_account();
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_SIGNING_KEY_SOURCE", "env");
    env.set("AUTH_CALLOUT_ISSUER_NKEY_SEED", kp.seed().expect("seed"));

    let source = signing_key_source_from_env(&env).expect("legacy fallback");
    assert_eq!(source.accepted().len(), 1);
}

#[test]
fn env_source_errors_when_secret_missing() {
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_SIGNING_KEY_SOURCE", "env");

    let err = signing_key_source_from_env(&env)
        .err()
        .expect("expected missing secret error");
    assert!(matches!(
        err,
        AuthCalloutError::MissingEnvVar("AUTH_CALLOUT_SIGNING_SECRET")
    ));
}

#[test]
fn file_source_loads_from_path() {
    let kp = KeyPair::new_account();
    let mut file = NamedTempFile::new().expect("temp file");
    file.write_all(kp.seed().expect("seed").as_bytes()).expect("write");

    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_SIGNING_KEY_SOURCE", "file");
    env.set("AUTH_CALLOUT_SIGNING_KEY_PATH", file.path().to_str().expect("path"));

    let source = signing_key_source_from_env(&env).expect("file source");
    assert_eq!(source.accepted().len(), 1);
}

#[test]
fn file_source_errors_when_path_missing() {
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_SIGNING_KEY_SOURCE", "file");

    let err = signing_key_source_from_env(&env)
        .err()
        .expect("expected missing path error");
    assert!(matches!(
        err,
        AuthCalloutError::MissingEnvVar("AUTH_CALLOUT_SIGNING_KEY_PATH")
    ));
}

#[test]
fn vault_source_is_not_configured() {
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_SIGNING_KEY_SOURCE", "vault");

    let err = signing_key_source_from_env(&env).err().expect("expected vault error");
    assert!(matches!(err, AuthCalloutError::VaultNotConfigured));
}

#[test]
fn unknown_source_kind_errors() {
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_SIGNING_KEY_SOURCE", "aws-kms");

    let err = signing_key_source_from_env(&env)
        .err()
        .expect("expected unknown source error");
    assert!(matches!(err, AuthCalloutError::UnknownSigningKeySource(kind) if kind == "aws-kms"));
}
