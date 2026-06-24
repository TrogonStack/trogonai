use super::*;
use crate::signing_key_source::{KeyVersion, SigningKeyHandle, StaticSigningKeySource};
use nkeys::KeyPair;
use serde_json::json;

#[test]
fn mint_decodes_expected_claims() {
    let issuer = KeyPair::new_account();
    let issuer_seed = issuer.seed().expect("issuer seed");
    let user = KeyPair::new_user();
    let material = MintingMaterial::new(
        SigningKey::from_seed(&issuer_seed).unwrap().keypair().clone(),
        KeyVersion::new("test").unwrap(),
    );
    let caller_id = CallerId::new("caller1").unwrap();
    let claims = UserJwtClaims {
        kid: material.version().clone(),
        sub: ExternalSubject::new("alice").unwrap(),
        aud: AccountName::new("tenant-acme"),
        data: SpiceDbPrincipal(json!({"spicedb_subject": "user/alice"})),
        nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
        caller_id,
    };
    let subject = UserJwtSubject::from_user_nkey(crate::wire::NkeyPublic::parse(user.public_key()).unwrap());
    let token = claims
        .mint_for_test_ttl(&material, &subject, Duration::from_secs(60))
        .unwrap();
    let handle = SigningKeyHandle::new(material.version().clone(), SigningKey::from_seed(&issuer_seed).unwrap());
    let decoded = UserJwtClaims::verify_with_handles(token.as_str(), &[handle]).unwrap();
    // `sub` round-trips the caller-supplied ExternalSubject ("alice"),
    // independent of the SpiceDB principal which can carry a different
    // shape ("user/alice"). See nats_user_jwt::ext_sub.
    assert_eq!(decoded.sub.as_str(), "alice");
    assert_eq!(decoded.aud.as_str(), "tenant-acme");
    assert_eq!(decoded.caller_id.as_str(), "caller1");
    assert_eq!(decoded.data.spicedb_subject().unwrap().as_str(), "user/alice");
}

#[test]
fn mint_rejects_wrong_verification_key() {
    let issuer_a = KeyPair::new_account();
    let issuer_a_seed = issuer_a.seed().expect("issuer seed");
    let issuer_b = KeyPair::new_account();
    let issuer_b_seed = issuer_b.seed().expect("issuer b seed");
    let user = KeyPair::new_user();
    let material = MintingMaterial::new(
        SigningKey::from_seed(&issuer_a_seed).unwrap().keypair().clone(),
        KeyVersion::new("test").unwrap(),
    );
    let caller_id = CallerId::new("cid").unwrap();
    let claims = UserJwtClaims {
        kid: material.version().clone(),
        sub: ExternalSubject::new("s").unwrap(),
        aud: AudienceAccount::new("a"),
        data: SpiceDbPrincipal(json!({})),
        nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
        caller_id,
    };
    let subject = UserJwtSubject::from_user_nkey(crate::wire::NkeyPublic::parse(user.public_key()).unwrap());
    let token = claims
        .mint_for_test_ttl(&material, &subject, Duration::from_secs(60))
        .unwrap();
    let wrong = SigningKeyHandle::new(
        KeyVersion::new("test").unwrap(),
        SigningKey::from_seed(&issuer_b_seed).unwrap(),
    );
    assert!(UserJwtClaims::verify_with_handles(token.as_str(), &[wrong]).is_err());
}

#[test]
fn caller_id_rejects_dots() {
    assert!(matches!(CallerId::new("a.b").unwrap_err(), JwtError::InvalidCallerId));
}

#[test]
fn external_subject_requires_non_empty() {
    assert!(matches!(
        ExternalSubject::new("").unwrap_err(),
        JwtError::InvalidExternalSubject
    ));
}

#[test]
fn spicedb_principal_prefers_custom_claim() {
    let v = json!({ "sub": "x", "spicedb_principal": { "kind": "special" } });
    let p = spicedb_principal_from_oidc_claims(&v);
    assert_eq!(p.0["kind"], "special");
}

#[test]
fn spicedb_subject_accessor_reads_claim() {
    let p = SpiceDbPrincipal::new("user/alice");
    assert_eq!(p.spicedb_subject().unwrap().as_str(), "user/alice");
}

#[test]
fn spicedb_subject_accessor_absent_when_missing_or_empty() {
    assert!(SpiceDbPrincipal(json!({})).spicedb_subject().is_none());
    assert!(
        SpiceDbPrincipal(json!({"spicedb_subject": ""}))
            .spicedb_subject()
            .is_none()
    );
}

#[test]
fn account_name_try_new_rejects_empty() {
    assert!(matches!(AccountName::try_new("").unwrap_err(), JwtError::Decode(_)));
}

#[test]
fn account_name_deserialize_rejects_empty() {
    let err = serde_json::from_str::<AccountName>(r#""""#).unwrap_err();
    assert!(err.to_string().contains("non-empty"));
}

#[test]
fn account_name_display_round_trips() {
    let account = AccountName::new("tenant-acme");
    assert_eq!(account.as_str(), "tenant-acme");
    assert_eq!(account.to_string(), "tenant-acme");
}

#[test]
fn caller_id_rejects_wildcards_and_whitespace() {
    for bad in ["", "a.b", "a*b", "a>b", "a b", "\ta"] {
        assert!(CallerId::new(bad).is_err(), "expected rejection for {bad:?}");
    }
}

#[test]
fn caller_id_deserialize_validates_segments() {
    assert!(serde_json::from_str::<CallerId>(r#""ok-caller""#).is_ok());
    assert!(serde_json::from_str::<CallerId>(r#""a.b""#).is_err());
}

#[test]
fn external_subject_deserialize_rejects_empty() {
    assert!(serde_json::from_str::<ExternalSubject>(r#""""#).is_err());
}

#[test]
fn minted_user_jwt_rejects_malformed_tokens() {
    assert!(matches!(MintedUserJwt::new("").unwrap_err(), JwtError::Decode(_)));
    assert!(matches!(
        MintedUserJwt::new("only.two").unwrap_err(),
        JwtError::Decode(_)
    ));
    assert!(MintedUserJwt::new("a.b.c").is_ok());
}

#[test]
fn minted_user_jwt_ensure_fresh_rejects_expired_token() {
    let issuer = KeyPair::new_account();
    let issuer_seed = issuer.seed().expect("issuer seed");
    let user = KeyPair::new_user();
    let material = MintingMaterial::new(
        SigningKey::from_seed(&issuer_seed).unwrap().keypair().clone(),
        KeyVersion::new("test").unwrap(),
    );
    let caller_id = CallerId::new("caller1").unwrap();
    let claims = UserJwtClaims {
        kid: material.version().clone(),
        sub: ExternalSubject::new("alice").unwrap(),
        aud: AccountName::new("tenant-acme"),
        data: SpiceDbPrincipal(json!({})),
        nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
        caller_id,
    };
    let subject = UserJwtSubject::from_user_nkey(crate::wire::NkeyPublic::parse(user.public_key()).unwrap());
    let token = claims
        .mint_for_test_ttl(&material, &subject, Duration::from_secs(60))
        .unwrap();
    assert!(matches!(
        token.ensure_fresh().unwrap_err(),
        JwtError::Decode(msg) if msg.contains("expired")
    ));
}

#[test]
fn verify_minted_user_jwt_rejects_wrong_audience() {
    let issuer = KeyPair::new_account();
    let issuer_seed = issuer.seed().expect("issuer seed");
    let user = KeyPair::new_user();
    let material = MintingMaterial::new(
        SigningKey::from_seed(&issuer_seed).unwrap().keypair().clone(),
        KeyVersion::new("test").unwrap(),
    );
    let caller_id = CallerId::new("caller1").unwrap();
    let claims = UserJwtClaims {
        kid: material.version().clone(),
        sub: ExternalSubject::new("alice").unwrap(),
        aud: AccountName::new("tenant-a"),
        data: SpiceDbPrincipal(json!({})),
        nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
        caller_id,
    };
    let subject = UserJwtSubject::from_user_nkey(crate::wire::NkeyPublic::parse(user.public_key()).unwrap());
    let token = claims
        .mint_for_test_ttl(&material, &subject, Duration::from_secs(3600))
        .unwrap();
    let source = StaticSigningKeySource::new(&issuer_seed, material.version().clone()).unwrap();
    let err =
        UserJwtClaims::verify_minted_user_jwt(token.as_str(), &source, &AccountName::new("tenant-b")).unwrap_err();
    assert!(matches!(err, JwtError::Decode(_)));
}

#[test]
fn caller_id_from_minted_jwt_reads_claim() {
    let issuer = KeyPair::new_account();
    let issuer_seed = issuer.seed().expect("issuer seed");
    let user = KeyPair::new_user();
    let material = MintingMaterial::new(
        SigningKey::from_seed(&issuer_seed).unwrap().keypair().clone(),
        KeyVersion::new("test").unwrap(),
    );
    let caller_id = CallerId::new("caller-from-jwt").unwrap();
    let claims = UserJwtClaims {
        kid: material.version().clone(),
        sub: ExternalSubject::new("alice").unwrap(),
        aud: AccountName::new("tenant-acme"),
        data: SpiceDbPrincipal(json!({})),
        nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
        caller_id: caller_id.clone(),
    };
    let subject = UserJwtSubject::from_user_nkey(crate::wire::NkeyPublic::parse(user.public_key()).unwrap());
    let token = claims
        .mint_for_test_ttl(&material, &subject, Duration::from_secs(3600))
        .unwrap();
    assert_eq!(
        caller_id_from_minted_jwt(token.as_str()).unwrap().as_str(),
        caller_id.as_str()
    );
}

#[test]
fn caller_id_from_minted_jwt_missing_claim_errors() {
    let err = caller_id_from_minted_jwt("a.b.c").unwrap_err();
    assert!(matches!(err, JwtError::Decode(_)));
}

#[test]
fn derive_caller_id_is_stable_per_subject_and_tenant() {
    let tenant = AccountName::new("tenant-acme");
    let first = derive_caller_id("alice", &tenant).unwrap();
    let second = derive_caller_id("alice", &tenant).unwrap();
    assert_eq!(first, second);
    assert_ne!(derive_caller_id("bob", &tenant).unwrap(), first);
}

#[test]
fn external_subject_from_der_prefixes_sha256() {
    let subject = external_subject_from_der("mtls", b"cert-bytes").unwrap();
    assert!(subject.as_str().starts_with("mtls|"));
    assert_eq!(subject.as_str().len(), "mtls|".len() + 64);
}

#[test]
fn spicedb_bundle_for_opaque_wraps_value() {
    let principal = spicedb_bundle_for_opaque(json!({"kind": "special"}));
    assert_eq!(principal.0["kind"], "special");
}

#[test]
fn spicedb_principal_from_oidc_falls_back_to_sub() {
    let principal = spicedb_principal_from_oidc_claims(&json!({"sub": "user/alice"}));
    assert_eq!(principal.spicedb_subject().unwrap().as_str(), "user/alice");
}

#[test]
fn signing_key_from_seed_rejects_invalid_seed() {
    assert!(matches!(
        SigningKey::from_seed("not-a-valid-seed").unwrap_err(),
        JwtError::InvalidSigningSeed(_)
    ));
}

#[test]
fn signing_key_clone_and_encoding_key_work() {
    let issuer = KeyPair::new_account();
    let seed = issuer.seed().expect("issuer seed");
    let key = SigningKey::from_seed(&seed).unwrap();
    let cloned = key.clone();
    let _ = cloned.encoding_key();
    let _ = key.encoding_key();
}

#[test]
fn jwt_error_converts_to_auth_callout_error() {
    let err: AuthCalloutError = JwtError::InvalidCallerId.into();
    assert!(matches!(err, AuthCalloutError::Jwt(JwtError::InvalidCallerId)));
}
