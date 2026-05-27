use jsonwebtoken::jwk::{AlgorithmParameters, JwkSet};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde_json::Value;

use crate::CLOCK_SKEW_SECS;
use crate::error::StsError;
use crate::types::VerifiedSubjectClaims;

pub fn verify_subject_token(token: &str, iss: &str, jwks: &JwkSet) -> Result<VerifiedSubjectClaims, StsError> {
    let header = decode_header(token).map_err(|e| StsError::InvalidGrant(format!("jwt header: {e}")))?;
    let alg = header.alg;
    let kid = header.kid.as_deref();

    let jwk = pick_jwk(jwks, alg, kid)?;
    let decoding_key = DecodingKey::from_jwk(jwk).map_err(|e| StsError::InvalidGrant(format!("jwks key: {e}")))?;

    let mut validation = Validation::new(alg);
    validation.leeway = CLOCK_SKEW_SECS as u64;
    validation.validate_exp = true;
    validation.validate_nbf = true;
    validation.validate_aud = false;
    validation.set_issuer(&[iss]);

    let token_data = decode::<Value>(token, &decoding_key, &validation)
        .map_err(|e| StsError::InvalidGrant(format!("subject_token invalid: {e}")))?;

    let claims = token_data.claims;
    let map = claims
        .as_object()
        .cloned()
        .ok_or_else(|| StsError::InvalidGrant("subject_token claims must be object".into()))?;

    let sub = string_claim(&map, "sub").ok_or_else(|| StsError::InvalidGrant("missing sub".into()))?;
    let token_iss = string_claim(&map, "iss").ok_or_else(|| StsError::InvalidGrant("missing iss".into()))?;
    if token_iss != iss {
        return Err(StsError::InvalidGrant(format!(
            "issuer mismatch: expected {iss}, got {token_iss}"
        )));
    }

    let exp = i64_claim(&map, "exp").ok_or_else(|| StsError::InvalidGrant("missing exp".into()))?;
    let iat = i64_claim(&map, "iat").unwrap_or(exp);

    let act_chain = map
        .get("act_chain")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    Ok(VerifiedSubjectClaims {
        sub,
        iss: token_iss,
        aud: map.get("aud").cloned().unwrap_or(Value::Null),
        exp,
        iat,
        agent_id: string_claim(&map, "agent_id"),
        agent_version: string_claim(&map, "agent_version"),
        wkl: string_claim(&map, "wkl"),
        purpose: string_claim(&map, "purpose"),
        scope: string_claim(&map, "scope"),
        tenant: string_claim(&map, "tenant"),
        act_chain,
        raw: map,
    })
}

fn pick_jwk<'a>(jwks: &'a JwkSet, alg: Algorithm, kid: Option<&str>) -> Result<&'a jsonwebtoken::jwk::Jwk, StsError> {
    let compatible: Vec<_> = jwks.keys.iter().filter(|k| jwk_compatible_with_alg(k, alg)).collect();
    if compatible.is_empty() {
        return Err(StsError::InvalidGrant(
            "JWKS contained no keys compatible with token algorithm".into(),
        ));
    }
    if let Some(kid) = kid
        && let Some(found) = compatible.iter().find(|k| k.common.key_id.as_deref() == Some(kid))
    {
        return Ok(found);
    }
    if compatible.len() == 1 {
        return Ok(compatible[0]);
    }
    Err(StsError::InvalidGrant("jwt kid did not match any JWKS keys".into()))
}

fn jwk_compatible_with_alg(jwk: &jsonwebtoken::jwk::Jwk, alg: Algorithm) -> bool {
    matches!(
        (&jwk.algorithm, alg),
        (AlgorithmParameters::RSA(_), Algorithm::RS256)
            | (AlgorithmParameters::EllipticCurve(_), Algorithm::ES256)
            | (AlgorithmParameters::EllipticCurve(_), Algorithm::ES384)
            | (AlgorithmParameters::OctetKey(_), Algorithm::HS256)
    )
}

fn string_claim(map: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    map.get(key).and_then(|v| match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        _ => None,
    })
}

fn i64_claim(map: &serde_json::Map<String, Value>, key: &str) -> Option<i64> {
    map.get(key).and_then(|v| match v {
        Value::Number(n) => n.as_i64(),
        _ => None,
    })
}
