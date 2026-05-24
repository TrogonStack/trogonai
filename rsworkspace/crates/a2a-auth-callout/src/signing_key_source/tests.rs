use std::io::Write;
use std::time::Duration;

use tempfile::NamedTempFile;

use crate::jwt::{CallerId, ExternalSubject, UserJwtClaims};
use crate::permissions::IssuedPermissions;
use super::env::test_env_dev_warn_count;
use super::{
    EnvSigningKeySource, FileSigningKeySource, KeyVersion, SigningKeySource, StaticSigningKeySource,
    VaultSigningKeySource,
};
use crate::{AccountName, SpiceDbPrincipal};

#[test]
fn env_source_current_previous_missing_and_warn_once() {
    unsafe {
        std::env::set_var("AUTH_CALLOUT_SIGNING_SECRET", "env-current-secret-bytes");
        std::env::set_var("AUTH_CALLOUT_SIGNING_SECRET_PREVIOUS", "env-previous-secret-bytes");
    }

    let before = test_env_dev_warn_count();
    let source = EnvSigningKeySource::from_env().expect("env source");
    let after_first = test_env_dev_warn_count();
    let accepted = source.accepted();
    assert_eq!(accepted.len(), 2);
    assert_eq!(accepted[0].version().as_str(), "current");
    assert_eq!(accepted[1].version().as_str(), "previous");

    let _ = EnvSigningKeySource::from_env().expect("second construction");
    let after_second = test_env_dev_warn_count();
    assert!(
        after_second <= after_first.max(before + 1),
        "dev-only warn must fire at most once per process"
    );

    unsafe {
        std::env::remove_var("AUTH_CALLOUT_SIGNING_SECRET");
    }
    let err = EnvSigningKeySource::from_env().unwrap_err();
    assert!(err.to_string().contains("AUTH_CALLOUT_SIGNING_SECRET"));
}

#[test]
fn file_reads_current_and_optional_previous() {
    let mut current = NamedTempFile::new().expect("current temp");
    current
        .write_all(b"file-current-secret----------------")
        .expect("write current");
    let source = FileSigningKeySource::new(current.path(), None::<&str>).expect("file source");
    assert_eq!(source.accepted().len(), 1);

    let mut previous = NamedTempFile::new().expect("previous temp");
    previous
        .write_all(b"file-previous-secret---------------")
        .expect("write previous");
    let source =
        FileSigningKeySource::new(current.path(), Some(previous.path())).expect("file overlap");
    assert_eq!(source.accepted().len(), 2);
}

#[test]
fn file_missing_current_errors() {
    let err = FileSigningKeySource::new("/no/such/signing-key-path", None::<&str>).unwrap_err();
    assert!(err.to_string().contains("failed to read signing key"));
}

#[test]
fn vault_load_always_errors() {
    let err = VaultSigningKeySource::load().unwrap_err();
    assert!(err.to_string().contains("vault source not implemented"));
}

#[test]
fn rotation_mint_verify_round_trip() {
    let source = StaticSigningKeySource::with_overlap(
        b"new-signing-secret----------------",
        KeyVersion::new("current").expect("version"),
        b"old-signing-secret-----------------",
        KeyVersion::new("previous").expect("version"),
    );

    let caller_id = CallerId::new("rotcaller").expect("caller");
    let claims = UserJwtClaims {
        kid: KeyVersion::new("previous").expect("version"),
        sub: ExternalSubject::new("subject").expect("sub"),
        aud: AccountName::new("tenant-acme"),
        data: SpiceDbPrincipal(serde_json::json!({"spicedb_subject": "subject"})),
        nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
        caller_id,
    };

    let old_handle = source
        .accepted()
        .into_iter()
        .find(|h| h.version().as_str() == "previous")
        .expect("previous handle");
    let old_token = claims
        .mint(
            &old_handle,
            std::time::UNIX_EPOCH + Duration::from_secs(2_000),
            Duration::from_secs(60),
        )
        .expect("mint with old key");

    let verified_old = UserJwtClaims::verify_with_source(old_token.as_str(), &source).expect("verify old");
    assert_eq!(verified_old.kid.as_str(), "previous");

    let current_handle = source.current();
    let mut current_claims = claims;
    current_claims.kid = current_handle.version().clone();
    let current_token = current_claims
        .mint(
            &current_handle,
            std::time::UNIX_EPOCH + Duration::from_secs(2_000),
            Duration::from_secs(60),
        )
        .expect("mint with current key");

    let verified_current =
        UserJwtClaims::verify_with_source(current_token.as_str(), &source).expect("verify current");
    assert_eq!(verified_current.kid.as_str(), "current");
}
