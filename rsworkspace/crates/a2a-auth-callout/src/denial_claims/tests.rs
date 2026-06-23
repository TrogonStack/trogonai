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
