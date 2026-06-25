use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde::Deserialize;

use super::*;
use crate::denial_category::DenialCategory;

#[derive(Debug, Deserialize)]
struct ParsedDenial {
    iss: String,
    aud: String,
    sub: String,
    nats: ParsedNats,
}

#[derive(Debug, Deserialize)]
struct ParsedNats {
    error: String,
    #[serde(rename = "type")]
    typ: String,
    version: u32,
    jwt: Option<String>,
}

fn sample_claims() -> DenialClaims {
    DenialClaims {
        iss: CalloutIssuer::new("ACALLOUTISSUER").unwrap(),
        aud: ServerAudience::new("ASERVERPUBKEY").unwrap(),
        sub: UserNkeySubject::new("UCLIENTNKEY").unwrap(),
        reason: DenialReason::new(DenialCategory::InvalidCredentials).unwrap(),
        request_jti: Some("REQJTI123".into()),
    }
}

#[test]
fn denial_jwt_round_trip() {
    let signing_key = SigningKey::from_secret(b"denial-test-secret--------------");
    let claims = sample_claims();
    let token = claims.mint_for_test(&signing_key, Duration::from_secs(60)).unwrap();

    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = false;
    validation.validate_aud = false;
    let decoded = decode::<ParsedDenial>(
        &token,
        &DecodingKey::from_secret(b"denial-test-secret--------------"),
        &validation,
    )
    .unwrap();

    assert_eq!(decoded.claims.iss, "ACALLOUTISSUER");
    assert_eq!(decoded.claims.aud, "ASERVERPUBKEY");
    assert_eq!(decoded.claims.sub, "UCLIENTNKEY");
    assert_eq!(decoded.claims.nats.error, "invalid_credentials");
    assert_eq!(decoded.claims.nats.typ, "authorization_response");
    assert_eq!(decoded.claims.nats.version, 2);
    assert!(decoded.claims.nats.jwt.is_none());
}

#[test]
fn denial_jwt_rejects_wrong_signing_key() {
    let signing_key = SigningKey::from_secret(b"signer-a------------------------");
    let token = sample_claims()
        .mint_for_test(&signing_key, Duration::from_secs(60))
        .unwrap();
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = false;
    validation.validate_aud = false;
    let err = decode::<ParsedDenial>(
        &token,
        &DecodingKey::from_secret(b"signer-b------------------------"),
        &validation,
    );
    assert!(err.is_err());
}

#[test]
fn callout_issuer_rejects_empty() {
    assert_eq!(CalloutIssuer::new("").unwrap_err(), CalloutIssuerError::Empty);
}

#[test]
fn server_audience_rejects_empty() {
    assert_eq!(ServerAudience::new("").unwrap_err(), ServerAudienceError::Empty);
}

#[test]
fn user_nkey_subject_rejects_empty() {
    assert_eq!(UserNkeySubject::new("").unwrap_err(), UserNkeySubjectError::Empty);
}

#[test]
fn denial_claims_error_converts_to_internal_auth_error() {
    let err: AuthCalloutError = DenialClaimsError::IssuedAtOutOfRange.into();
    assert!(matches!(err, AuthCalloutError::Internal(message) if message.contains("issued-at")));
}

#[test]
fn mint_rejects_time_before_unix_epoch() {
    let signing_key = SigningKey::from_secret(b"denial-test-secret--------------");
    let claims = sample_claims();
    let err = claims
        .mint(
            &signing_key,
            UNIX_EPOCH - Duration::from_secs(1),
            Duration::from_secs(60),
        )
        .unwrap_err();
    assert!(matches!(err, DenialClaimsError::SystemTime(_)));
}
