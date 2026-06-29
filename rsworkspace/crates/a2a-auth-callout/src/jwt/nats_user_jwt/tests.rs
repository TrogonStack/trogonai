use std::time::SystemTime;

use super::super::{MintedUserJwt, SigningKey, UserJwtClaims, spicedb_principal_from_oidc_claims};
use super::*;
use crate::permissions::IssuedPermissions;
use crate::signing_key_source::{KeyVersion, SigningKeyHandle, StaticSigningKeySource};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use nkeys::KeyPair;
use serde_json::{Value, json};

fn fixture_material() -> (MintingMaterial, String, KeyPair, KeyPair) {
    let issuer = KeyPair::new_account();
    let user = KeyPair::new_user();
    let issuer_seed = issuer.seed().expect("issuer seed");
    let material = MintingMaterial::new(
        KeyPair::from_seed(&issuer_seed).unwrap(),
        KeyVersion::new("test").unwrap(),
    );
    (material, issuer_seed, issuer, user)
}

fn fixture_claims(material: &MintingMaterial) -> (UserJwtClaims, UserJwtSubject, KeyPair) {
    let user = KeyPair::new_user();
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
    (claims, subject, user)
}

fn mint_token(
    claims: &UserJwtClaims,
    material: &MintingMaterial,
    subject: &UserJwtSubject,
    issued_at: SystemTime,
    ttl: Duration,
) -> String {
    mint_nats_user_jwt(claims, material, subject, issued_at, ttl)
        .unwrap()
        .into_string()
}

fn resign_nats_user_jwt(
    token: &str,
    material: &MintingMaterial,
    mutate: impl FnOnce(&mut NatsJwtHeader, &mut Value),
) -> String {
    let parts: Vec<&str> = token.split('.').collect();
    let mut header: NatsJwtHeader = decode_segment(parts[0]).unwrap();
    let mut payload: Value = decode_segment(parts[1]).unwrap();
    mutate(&mut header, &mut payload);
    let hdr = encode_segment(&header).unwrap();
    let claims_segment = encode_segment(&payload).unwrap();
    let signing_input = format!("{hdr}.{claims_segment}");
    let sig = material.issuer_keypair().sign(signing_input.as_bytes()).unwrap();
    format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(sig))
}

fn decode_err_contains(err: JwtError, needle: &str) -> bool {
    matches!(err, JwtError::Decode(msg) if msg.contains(needle))
}

#[test]
fn minted_jwt_has_nats_header_and_verifies() {
    let (material, issuer_seed, issuer, user) = fixture_material();
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
    let token = mint_nats_user_jwt(
        &claims,
        &material,
        &subject,
        UNIX_EPOCH + Duration::from_secs(2_000),
        Duration::from_secs(60),
    )
    .unwrap();

    let header: NatsJwtHeader = decode_segment(token.as_str().split('.').next().unwrap()).unwrap();
    assert_eq!(header.algorithm, HEADER_ALGORITHM);
    assert_eq!(header.kid.as_deref(), Some("test"));

    let payload: Value = decode_nats_user_payload(token.as_str()).unwrap();
    assert_eq!(payload["sub"], user.public_key());
    assert_eq!(payload["iss"], issuer.public_key());
    assert_eq!(payload["aud"], "tenant-acme");
    assert_eq!(payload["caller_id"], "caller1");
    assert_eq!(payload["nats"]["type"], "user");
    assert_eq!(payload["nats"]["pub"]["allow"][0], "a2a.gateway.>");

    let handle = SigningKeyHandle::new(material.version().clone(), SigningKey::from_seed(&issuer_seed).unwrap());
    let verified = verify_with_material(token.as_str(), &handle.minting_material()).unwrap();
    assert_eq!(verified.caller_id.as_str(), "caller1");
    assert_eq!(verified.aud.as_str(), "tenant-acme");
}

#[test]
fn account_name_try_new_rejects_empty() {
    assert!(matches!(AccountName::try_new("").unwrap_err(), JwtError::Decode(_)));
}

#[test]
fn verify_rejects_empty_handle_list() {
    let err = verify_nats_user_jwt("a.b.c", &[]).unwrap_err();
    assert!(matches!(err, JwtError::NoSigningKeyForKid));
}

#[test]
fn decode_nats_user_payload_rejects_invalid_segment_count() {
    let err = decode_nats_user_payload("only-one-part").unwrap_err();
    assert!(matches!(err, JwtError::Decode(_)));
}

#[test]
fn verify_with_source_uses_accepted_handles() {
    let (material, issuer_seed, _, user) = fixture_material();
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
    let token = mint_nats_user_jwt(
        &claims,
        &material,
        &subject,
        UNIX_EPOCH + Duration::from_secs(2_000),
        Duration::from_secs(60),
    )
    .unwrap();
    let source = StaticSigningKeySource::new(&issuer_seed, material.version().clone()).unwrap();
    let verified = verify_nats_user_jwt_with_source(token.as_str(), &source).unwrap();
    assert_eq!(verified.caller_id.as_str(), "caller1");
}

#[test]
fn verify_nats_user_jwt_selects_handle_by_header_kid() {
    let (material, issuer_seed, _, _) = fixture_material();
    let (claims, subject, _) = fixture_claims(&material);
    let token = mint_token(
        &claims,
        &material,
        &subject,
        UNIX_EPOCH + Duration::from_secs(2_000),
        Duration::from_secs(60),
    );
    let other = KeyPair::new_account();
    let other_seed = other.seed().expect("other seed");
    let wrong_handle = SigningKeyHandle::new(
        KeyVersion::new("other").unwrap(),
        SigningKey::from_seed(&other_seed).unwrap(),
    );
    let right_handle = SigningKeyHandle::new(material.version().clone(), SigningKey::from_seed(&issuer_seed).unwrap());
    let verified = verify_nats_user_jwt(token.as_str(), &[wrong_handle, right_handle]).unwrap();
    assert_eq!(verified.sub.as_str(), "alice");
}

#[test]
fn verify_nats_user_jwt_falls_back_when_header_kid_unknown() {
    let (material, issuer_seed, _, _) = fixture_material();
    let (claims, subject, _) = fixture_claims(&material);
    let token = mint_token(
        &claims,
        &material,
        &subject,
        UNIX_EPOCH + Duration::from_secs(2_000),
        Duration::from_secs(60),
    );
    let tampered = resign_nats_user_jwt(&token, &material, |header, _| {
        header.kid = Some("unknown-kid".into());
    });
    let handle = SigningKeyHandle::new(material.version().clone(), SigningKey::from_seed(&issuer_seed).unwrap());
    let verified = verify_nats_user_jwt(tampered.as_str(), &[handle]).unwrap();
    assert_eq!(verified.aud.as_str(), "tenant-acme");
}

#[test]
fn verify_nats_user_jwt_returns_last_verification_error() {
    let (material, _, _, _) = fixture_material();
    let (claims, subject, _) = fixture_claims(&material);
    let token = mint_token(
        &claims,
        &material,
        &subject,
        UNIX_EPOCH + Duration::from_secs(2_000),
        Duration::from_secs(60),
    );
    let other = KeyPair::new_account();
    let other_seed = other.seed().expect("other seed");
    let wrong = SigningKeyHandle::new(
        KeyVersion::new("wrong").unwrap(),
        SigningKey::from_seed(&other_seed).unwrap(),
    );
    let err = verify_nats_user_jwt(token.as_str(), &[wrong]).unwrap_err();
    assert!(matches!(err, JwtError::Decode(_)));
}

#[test]
fn verify_with_material_rejects_invalid_segment_count() {
    let (material, issuer_seed, _, _) = fixture_material();
    let handle = SigningKeyHandle::new(material.version().clone(), SigningKey::from_seed(&issuer_seed).unwrap());
    let err = verify_with_material("only.two", &handle.minting_material()).unwrap_err();
    assert!(decode_err_contains(err, "invalid JWT segment count"));
}

#[test]
fn verify_with_material_rejects_unsupported_header() {
    let (material, issuer_seed, _, _) = fixture_material();
    let (claims, subject, _) = fixture_claims(&material);
    let token = mint_token(
        &claims,
        &material,
        &subject,
        UNIX_EPOCH + Duration::from_secs(2_000),
        Duration::from_secs(60),
    );
    let tampered = resign_nats_user_jwt(&token, &material, |header, _| {
        header.header_type = "JWS".into();
    });
    let handle = SigningKeyHandle::new(material.version().clone(), SigningKey::from_seed(&issuer_seed).unwrap());
    let err = verify_with_material(tampered.as_str(), &handle.minting_material()).unwrap_err();
    assert!(decode_err_contains(err, "unsupported NATS user JWT header"));
}

#[test]
fn verify_with_material_rejects_issuer_mismatch() {
    let (material, issuer_seed, _, _) = fixture_material();
    let (claims, subject, _) = fixture_claims(&material);
    let token = mint_token(
        &claims,
        &material,
        &subject,
        UNIX_EPOCH + Duration::from_secs(2_000),
        Duration::from_secs(60),
    );
    let tampered = resign_nats_user_jwt(&token, &material, |_, payload| {
        payload["iss"] = json!("wrong-issuer");
    });
    let handle = SigningKeyHandle::new(material.version().clone(), SigningKey::from_seed(&issuer_seed).unwrap());
    let err = verify_with_material(tampered.as_str(), &handle.minting_material()).unwrap_err();
    assert!(decode_err_contains(err, "issuer does not match signing key"));
}

#[test]
fn verify_with_material_falls_back_to_spicedb_subject_without_ext_sub() {
    let (material, issuer_seed, _, _) = fixture_material();
    let (claims, subject, _) = fixture_claims(&material);
    let token = mint_token(
        &claims,
        &material,
        &subject,
        UNIX_EPOCH + Duration::from_secs(2_000),
        Duration::from_secs(60),
    );
    let legacy = resign_nats_user_jwt(&token, &material, |_, payload| {
        payload.as_object_mut().unwrap().remove("ext_sub");
    });
    let handle = SigningKeyHandle::new(material.version().clone(), SigningKey::from_seed(&issuer_seed).unwrap());
    let verified = verify_with_material(legacy.as_str(), &handle.minting_material()).unwrap();
    assert_eq!(verified.sub.as_str(), "user/alice");
}

#[test]
fn verify_with_material_rejects_missing_external_subject() {
    let (material, issuer_seed, _, _) = fixture_material();
    let (claims, subject, _) = fixture_claims(&material);
    let token = mint_token(
        &claims,
        &material,
        &subject,
        UNIX_EPOCH + Duration::from_secs(2_000),
        Duration::from_secs(60),
    );
    let stripped = resign_nats_user_jwt(&token, &material, |_, payload| {
        payload.as_object_mut().unwrap().remove("ext_sub");
        payload["data"] = json!({});
    });
    let handle = SigningKeyHandle::new(material.version().clone(), SigningKey::from_seed(&issuer_seed).unwrap());
    let err = verify_with_material(stripped.as_str(), &handle.minting_material()).unwrap_err();
    assert!(decode_err_contains(err, "missing ext_sub / spicedb_subject"));
}

#[test]
fn verify_nats_user_jwt_rejects_invalid_segment_count() {
    let (material, issuer_seed, _, _) = fixture_material();
    let handle = SigningKeyHandle::new(material.version().clone(), SigningKey::from_seed(&issuer_seed).unwrap());
    let err = verify_nats_user_jwt("only-one-part", &[handle]).unwrap_err();
    assert!(decode_err_contains(err, "invalid JWT segment count"));
}

#[test]
fn verify_nats_user_jwt_rejects_unsupported_header_algorithm() {
    let (material, issuer_seed, _, _) = fixture_material();
    let (claims, subject, _) = fixture_claims(&material);
    let token = mint_token(
        &claims,
        &material,
        &subject,
        UNIX_EPOCH + Duration::from_secs(2_000),
        Duration::from_secs(60),
    );
    let tampered = resign_nats_user_jwt(&token, &material, |header, _| {
        header.algorithm = "HS256".into();
    });
    let handle = SigningKeyHandle::new(material.version().clone(), SigningKey::from_seed(&issuer_seed).unwrap());
    let err = verify_nats_user_jwt(tampered.as_str(), &[handle]).unwrap_err();
    assert!(decode_err_contains(err, "unsupported JWT algorithm"));
}

#[test]
fn verify_minted_user_jwt_accepts_fresh_token() {
    let (material, issuer_seed, _, _) = fixture_material();
    let (claims, subject, _) = fixture_claims(&material);
    let token = mint_token(
        &claims,
        &material,
        &subject,
        SystemTime::now(),
        Duration::from_secs(3600),
    );
    let source = StaticSigningKeySource::new(&issuer_seed, material.version().clone()).unwrap();
    let verified =
        UserJwtClaims::verify_minted_user_jwt(token.as_str(), &source, &AccountName::new("tenant-acme")).unwrap();
    assert_eq!(verified.caller_id.as_str(), "caller1");
}

#[test]
fn minted_user_jwt_ensure_fresh_rejects_not_yet_valid() {
    let (material, _, _, _) = fixture_material();
    let (claims, subject, _) = fixture_claims(&material);
    let token = mint_token(
        &claims,
        &material,
        &subject,
        SystemTime::now(),
        Duration::from_secs(3600),
    );
    let future_nbf = resign_nats_user_jwt(&token, &material, |_, payload| {
        payload["nbf"] = json!(i64::MAX);
    });
    let minted = MintedUserJwt::new(future_nbf).unwrap();
    let err = minted.ensure_fresh().unwrap_err();
    assert!(decode_err_contains(err, "not yet valid"));
}

#[test]
fn minted_user_jwt_into_string_round_trips() {
    let (material, _, _, _) = fixture_material();
    let (claims, subject, _) = fixture_claims(&material);
    let token = mint_nats_user_jwt(
        &claims,
        &material,
        &subject,
        UNIX_EPOCH + Duration::from_secs(2_000),
        Duration::from_secs(60),
    )
    .unwrap();
    let raw = token.as_str().to_owned();
    assert_eq!(token.into_string(), raw);
}

#[test]
fn spicedb_principal_from_oidc_claims_empty_fallback() {
    let principal = spicedb_principal_from_oidc_claims(&json!({}));
    assert!(principal.spicedb_subject().is_none());
}

#[test]
fn signing_key_from_secret_has_no_nkey_keypair() {
    let key = SigningKey::from_secret(b"test-secret");
    assert!(std::panic::catch_unwind(|| key.keypair()).is_err());
}
