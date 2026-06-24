use std::io::Write;

use tempfile::NamedTempFile;

use super::signing_key_source_from_process_env;
use crate::error::AuthCalloutError;
use crate::signing_key_source::env_test_lock;
use nkeys::KeyPair;

fn with_env_loader_tests<F: FnOnce()>(f: F) {
    let _guard = env_test_lock();
    f();
}

#[test]
fn env_source_loads_from_signing_secret() {
    with_env_loader_tests(|| {
        let kp = KeyPair::new_account();
        unsafe {
            std::env::set_var("AUTH_CALLOUT_SIGNING_KEY_SOURCE", "env");
            std::env::set_var("AUTH_CALLOUT_SIGNING_SECRET", kp.seed().expect("seed"));
            std::env::remove_var("AUTH_CALLOUT_ISSUER_NKEY_SEED");
        }

        let source = signing_key_source_from_process_env().expect("env source");
        assert_eq!(source.accepted().len(), 1);
    });
}

#[test]
fn env_source_falls_back_to_legacy_issuer_seed() {
    with_env_loader_tests(|| {
        let kp = KeyPair::new_account();
        unsafe {
            std::env::set_var("AUTH_CALLOUT_SIGNING_KEY_SOURCE", "env");
            std::env::remove_var("AUTH_CALLOUT_SIGNING_SECRET");
            std::env::set_var("AUTH_CALLOUT_ISSUER_NKEY_SEED", kp.seed().expect("seed"));
        }

        let source = signing_key_source_from_process_env().expect("legacy fallback");
        assert_eq!(source.accepted().len(), 1);
    });
}

#[test]
fn env_source_errors_when_secret_missing() {
    with_env_loader_tests(|| {
        unsafe {
            std::env::set_var("AUTH_CALLOUT_SIGNING_KEY_SOURCE", "env");
            std::env::remove_var("AUTH_CALLOUT_SIGNING_SECRET");
            std::env::remove_var("AUTH_CALLOUT_ISSUER_NKEY_SEED");
        }

        let err = signing_key_source_from_process_env()
            .err()
            .expect("expected missing secret error");
        assert!(matches!(
            err,
            AuthCalloutError::MissingEnvVar("AUTH_CALLOUT_SIGNING_SECRET")
        ));
    });
}

#[test]
fn file_source_loads_from_path() {
    with_env_loader_tests(|| {
        let kp = KeyPair::new_account();
        let mut file = NamedTempFile::new().expect("temp file");
        file.write_all(kp.seed().expect("seed").as_bytes()).expect("write");

        unsafe {
            std::env::set_var("AUTH_CALLOUT_SIGNING_KEY_SOURCE", "file");
            std::env::set_var("AUTH_CALLOUT_SIGNING_KEY_PATH", file.path());
            std::env::remove_var("AUTH_CALLOUT_SIGNING_KEY_PREVIOUS_PATH");
        }

        let source = signing_key_source_from_process_env().expect("file source");
        assert_eq!(source.accepted().len(), 1);
    });
}

#[test]
fn file_source_errors_when_path_missing() {
    with_env_loader_tests(|| {
        unsafe {
            std::env::set_var("AUTH_CALLOUT_SIGNING_KEY_SOURCE", "file");
            std::env::remove_var("AUTH_CALLOUT_SIGNING_KEY_PATH");
        }

        let err = signing_key_source_from_process_env()
            .err()
            .expect("expected missing path error");
        assert!(matches!(
            err,
            AuthCalloutError::MissingEnvVar("AUTH_CALLOUT_SIGNING_KEY_PATH")
        ));
    });
}

#[test]
fn vault_source_is_not_configured() {
    with_env_loader_tests(|| {
        unsafe {
            std::env::set_var("AUTH_CALLOUT_SIGNING_KEY_SOURCE", "vault");
        }

        let err = signing_key_source_from_process_env()
            .err()
            .expect("expected vault error");
        assert!(matches!(err, AuthCalloutError::VaultNotConfigured));
    });
}

#[test]
fn unknown_source_kind_errors() {
    with_env_loader_tests(|| {
        unsafe {
            std::env::set_var("AUTH_CALLOUT_SIGNING_KEY_SOURCE", "aws-kms");
        }

        let err = signing_key_source_from_process_env()
            .err()
            .expect("expected unknown source error");
        assert!(matches!(err, AuthCalloutError::UnknownSigningKeySource(kind) if kind == "aws-kms"));
    });
}
