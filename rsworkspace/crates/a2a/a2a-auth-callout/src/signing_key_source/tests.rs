use std::io::Write;
use std::time::Duration;

use tempfile::NamedTempFile;
use trogon_identity_types::PrincipalKind;
use trogon_std::env::InMemoryEnv;

use super::env::test_env_dev_warn_count;
use super::{
    EnvSigningKeySource, FileSigningKeySource, IssuerPublicKeys, IssuerPublicKeysError, KeyVersion, KeyVersionError,
    MintingMaterial, SigningKeyHandle, SigningKeySource, StaticSigningKeySource, VaultSigningKeySource,
    VerifyingKeySource,
};
use crate::admission_keys::{AccountPublicKeyError, IssuerPublicKey};
use crate::error::AuthCalloutError;
use crate::jwt::{CallerId, ExternalSubject, SigningKey, UserJwtClaims, UserJwtSubject};
use crate::permissions::IssuedPermissions;
use crate::{AccountName, SpiceDbPrincipal};
use nkeys::KeyPair;

#[test]
fn env_source_current_previous_missing_and_warn_once() {
    let account = KeyPair::new_account();
    let previous = KeyPair::new_account();
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_SIGNING_SECRET", account.seed().expect("account seed"));
    env.set(
        "AUTH_CALLOUT_SIGNING_SECRET_PREVIOUS",
        previous.seed().expect("previous seed"),
    );

    let before = test_env_dev_warn_count();
    let source = EnvSigningKeySource::from_env(&env).expect("env source");
    let after_first = test_env_dev_warn_count();
    let accepted = source.accepted();
    assert_eq!(accepted.len(), 2);
    assert_eq!(accepted[0].version().as_str(), "current");
    assert_eq!(accepted[1].version().as_str(), "previous");

    let _ = EnvSigningKeySource::from_env(&env).expect("second construction");
    let after_second = test_env_dev_warn_count();
    assert!(
        after_second <= after_first.max(before + 1),
        "dev-only warn must fire at most once per process"
    );

    let env2 = InMemoryEnv::new();
    let err = EnvSigningKeySource::from_env(&env2).unwrap_err();
    assert!(matches!(
        err,
        AuthCalloutError::MissingEnvVar("AUTH_CALLOUT_SIGNING_SECRET")
    ));
}

#[test]
fn env_source_current_returns_current_handle() {
    let account = KeyPair::new_account();
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_SIGNING_SECRET", account.seed().expect("account seed"));

    let source = EnvSigningKeySource::from_env(&env).expect("env source");
    assert_eq!(source.current().version().as_str(), "current");
}

#[test]
fn env_source_ignores_empty_previous_secret() {
    let account = KeyPair::new_account();
    let env = InMemoryEnv::new();
    env.set("AUTH_CALLOUT_SIGNING_SECRET", account.seed().expect("account seed"));
    env.set("AUTH_CALLOUT_SIGNING_SECRET_PREVIOUS", "");

    let source = EnvSigningKeySource::from_env(&env).expect("env source");
    assert_eq!(source.accepted().len(), 1);
    assert_eq!(source.accepted()[0].version().as_str(), "current");
}

#[test]
fn file_reads_current_and_optional_previous() {
    let current_kp = KeyPair::new_account();
    let mut current = NamedTempFile::new().expect("current temp");
    current
        .write_all(current_kp.seed().expect("current seed").as_bytes())
        .expect("write current");
    let source = FileSigningKeySource::new(current.path(), None::<&str>).expect("file source");
    assert_eq!(source.accepted().len(), 1);
    assert_eq!(source.current().version().as_str(), "current");

    let previous_kp = KeyPair::new_account();
    let mut previous = NamedTempFile::new().expect("previous temp");
    previous
        .write_all(previous_kp.seed().expect("previous seed").as_bytes())
        .expect("write previous");
    let source = FileSigningKeySource::new(current.path(), Some(previous.path())).expect("file overlap");
    let accepted = source.accepted();
    assert_eq!(accepted.len(), 2);
    assert_eq!(accepted[0].version().as_str(), "current");
    assert_eq!(accepted[1].version().as_str(), "previous");
}

#[test]
fn file_missing_current_errors() {
    let err = FileSigningKeySource::new("/no/such/signing-key-path", None::<&str>).unwrap_err();
    assert!(matches!(err, AuthCalloutError::KeyLoadIo { .. }));
}

#[test]
fn vault_load_always_errors() {
    let err = VaultSigningKeySource::load().unwrap_err();
    assert!(matches!(err, AuthCalloutError::VaultNotConfigured));
}

#[test]
fn rotation_mint_verify_round_trip() {
    let old_kp = KeyPair::new_account();
    let current_kp = KeyPair::new_account();
    let user = KeyPair::new_user();
    let source = StaticSigningKeySource::with_overlap(
        &current_kp.seed().expect("current seed"),
        KeyVersion::new("current").expect("version"),
        &old_kp.seed().expect("previous seed"),
        KeyVersion::new("previous").expect("version"),
    )
    .expect("overlap source");

    let caller_id = CallerId::new("rotcaller").expect("caller");
    let claims = UserJwtClaims {
        kid: KeyVersion::new("previous").expect("version"),
        sub: ExternalSubject::new("subject").expect("sub"),
        aud: AccountName::new("tenant-acme"),
        data: SpiceDbPrincipal(serde_json::json!({"spicedb_subject": "subject"})),
        nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
        caller_id,
        principal_kind: PrincipalKind::Person,
    };

    let old_handle = source
        .accepted()
        .into_iter()
        .find(|h| h.version().as_str() == "previous")
        .expect("previous handle");
    let subject = UserJwtSubject::from_user_nkey(crate::wire::NkeyPublic::parse(user.public_key()).unwrap());
    let old_token = claims
        .mint(
            &old_handle.minting_material(),
            &subject,
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
            &current_handle.minting_material(),
            &subject,
            std::time::UNIX_EPOCH + Duration::from_secs(2_000),
            Duration::from_secs(60),
        )
        .expect("mint with current key");

    let verified_current = UserJwtClaims::verify_with_source(current_token.as_str(), &source).expect("verify current");
    assert_eq!(verified_current.kid.as_str(), "current");
}

#[test]
fn key_version_rejects_empty_and_formats() {
    assert!(matches!(KeyVersion::new("").unwrap_err(), KeyVersionError::Empty));
    let version = KeyVersion::new("v1").unwrap();
    assert_eq!(version.as_str(), "v1");
    assert_eq!(version.to_string(), "v1");
}

#[test]
fn minting_material_exposes_issuer_public_and_debug() {
    let issuer = KeyPair::new_account();
    let version = KeyVersion::new("current").unwrap();
    let material = MintingMaterial::new(issuer.clone(), version);
    assert_eq!(material.issuer_public(), issuer.public_key());
    let debug = format!("{material:?}");
    assert!(debug.contains("MintingMaterial"));
    assert!(debug.contains("current"));
}

#[test]
fn signing_key_handle_minting_material_roundtrip() {
    let issuer = KeyPair::new_account();
    let version = KeyVersion::new("current").unwrap();
    let handle = SigningKeyHandle::new(version.clone(), SigningKey::from_seed(issuer.seed().unwrap()).unwrap());
    let material = handle.minting_material();
    assert_eq!(material.version(), &version);
    assert_eq!(material.issuer_public(), issuer.public_key());
    let debug = format!("{handle:?}");
    assert!(debug.contains("SigningKeyHandle"));
}

fn minted_by(issuer: &KeyPair, aud: &str) -> String {
    let user = KeyPair::new_user();
    let caller_id = CallerId::new("pubcaller").expect("caller");
    let claims = UserJwtClaims {
        kid: KeyVersion::new("current").expect("version"),
        sub: ExternalSubject::new("subject").expect("sub"),
        aud: AccountName::new(aud),
        data: SpiceDbPrincipal(serde_json::json!({"spicedb_subject": "subject"})),
        nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
        caller_id,
        principal_kind: PrincipalKind::Person,
    };
    let material = MintingMaterial::new(issuer.clone(), KeyVersion::new("current").expect("version"));
    let subject = UserJwtSubject::from_user_nkey(crate::wire::NkeyPublic::parse(user.public_key()).unwrap());
    claims
        .mint(
            &material,
            &subject,
            std::time::SystemTime::now(),
            Duration::from_secs(60),
        )
        .expect("mint")
        .as_str()
        .to_owned()
}

#[test]
fn issuer_public_keys_verify_a_minted_token_without_any_seed() {
    let issuer = KeyPair::new_account();
    let token = minted_by(&issuer, "tenant-acme");
    let keys = IssuerPublicKeys::parse(&issuer.public_key()).expect("public keys");

    let verified = UserJwtClaims::verify_minted_user_jwt(&token, &keys, &AccountName::new("tenant-acme"))
        .expect("verify with the public key alone");
    assert_eq!(verified.caller_id.as_str(), "pubcaller");
}

#[test]
fn issuer_public_keys_reject_a_token_from_another_issuer() {
    let token = minted_by(&KeyPair::new_account(), "tenant-acme");
    let keys = IssuerPublicKeys::parse(&KeyPair::new_account().public_key()).expect("public keys");

    let err = UserJwtClaims::verify_minted_user_jwt(&token, &keys, &AccountName::new("tenant-acme")).unwrap_err();
    assert!(matches!(err, crate::jwt::JwtError::Decode(_)));
}

#[test]
fn issuer_public_keys_accept_the_previous_key_during_rotation() {
    let current = KeyPair::new_account();
    let previous = KeyPair::new_account();
    let keys =
        IssuerPublicKeys::parse(&format!("{}, {}", current.public_key(), previous.public_key())).expect("public keys");

    for issuer in [&current, &previous] {
        let token = minted_by(issuer, "tenant-acme");
        UserJwtClaims::verify_minted_user_jwt(&token, &keys, &AccountName::new("tenant-acme")).expect("accepted");
    }
    let retired = minted_by(&KeyPair::new_account(), "tenant-acme");
    assert!(UserJwtClaims::verify_minted_user_jwt(&retired, &keys, &AccountName::new("tenant-acme")).is_err());
}

#[test]
fn signing_key_sources_provide_verifying_keys_for_the_keys_they_mint_with() {
    let current = KeyPair::new_account();
    let previous = KeyPair::new_account();
    let source = StaticSigningKeySource::with_overlap(
        &current.seed().expect("seed"),
        KeyVersion::new("current").expect("version"),
        &previous.seed().expect("seed"),
        KeyVersion::new("previous").expect("version"),
    )
    .expect("overlap source");

    let keys = source.accepted_verifying_keys();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0].issuer_public(), current.public_key());
    assert_eq!(keys[0].version().map(KeyVersion::as_str), Some("current"));
    assert_eq!(keys[1].issuer_public(), previous.public_key());
}

#[test]
fn issuer_public_keys_reject_seeds_and_non_account_keys() {
    let account = KeyPair::new_account();
    let seed =
        IssuerPublicKeys::parse(&format!("{},{}", account.public_key(), account.seed().expect("seed"))).unwrap_err();
    assert_eq!(
        seed,
        IssuerPublicKeysError::Invalid {
            position: 2,
            source: AccountPublicKeyError::NotAPublicKey,
        }
    );
    assert_eq!(
        seed.to_string(),
        "entry 2 is not an account public key: it is not an NKey public key"
    );

    let user = IssuerPublicKeys::parse(&KeyPair::new_user().public_key()).unwrap_err();
    assert_eq!(
        user,
        IssuerPublicKeysError::Invalid {
            position: 1,
            source: AccountPublicKeyError::NotAnAccount,
        }
    );
}

#[test]
fn issuer_public_keys_reject_empty_and_duplicate_lists() {
    assert_eq!(IssuerPublicKeys::parse("").unwrap_err(), IssuerPublicKeysError::Empty);
    assert_eq!(
        IssuerPublicKeys::parse(" , ,").unwrap_err(),
        IssuerPublicKeysError::Empty
    );

    let account = KeyPair::new_account().public_key();
    let duplicate = IssuerPublicKeys::parse(&format!("{account},{account}")).unwrap_err();
    assert_eq!(
        duplicate,
        IssuerPublicKeysError::Duplicate(IssuerPublicKey::parse(&account).expect("account key"))
    );
    assert_eq!(
        duplicate.to_string(),
        format!("lists issuer public key {account} more than once")
    );
}
