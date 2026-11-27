use serde_json::{Map, Value};
use trogon_sandbox_contract::JwtMasking;

use crate::constants::FAKE_EXPIRY;
use crate::constants::FAKE_SIGNATURE;
use crate::jwt_structure::{ParsedJwt, encode, parse};
use crate::{JwtMaskError, MaskedClaim, MaskedJwt, RealValue, Sentinel};

fn assemble(header: &str, payload: &Value) -> Sentinel {
    Sentinel::from_fake_jwt(format!("{header}.{}.{FAKE_SIGNATURE}", encode(payload)))
}

fn generic_fake() -> Sentinel {
    let plain = Sentinel::mint(&RealValue::new(String::new()));
    let header = encode(&Value::Object(Map::from_iter([
        ("alg".to_string(), Value::String("HS256".to_string())),
        ("typ".to_string(), Value::String("JWT".to_string())),
    ])));
    let payload = Value::Object(Map::from_iter([
        ("sub".to_string(), Value::String(plain.as_str().to_string())),
        ("exp".to_string(), Value::from(FAKE_EXPIRY)),
    ]));
    assemble(&header, &payload)
}

/// Masks the JWT `real` so the sandbox sees a token it can parse that proves nothing.
///
/// The fake is `header.payload.filler`. Without named claims the header is a fixed `HS256` one and the payload carries
/// a plain sentinel as `sub`. With named claims the real header is kept verbatim and each named top-level string claim
/// in the payload is replaced by its own sentinel. The real signature is never copied: over an edited payload it
/// would let the sandbox verify guesses of a low-entropy claim offline. The header says `HS256`, never `none`, so a
/// verifier handed the fake unswapped rejects the filler signature instead of accepting an unsigned token.
///
/// Anything that is not verifiably a JWT, or names claims it does not hold, is an error.
pub fn mask_jwt(masking: &JwtMasking, real: &RealValue) -> Result<MaskedJwt, JwtMaskError> {
    mask_jwt_reusing(masking, real, &|_| None)
}

/// [`mask_jwt`] reusing the sentinel `reuse(index)` returns for claim `index`, so a re-registration keeps the
/// sentinels a process already read.
pub(crate) fn mask_jwt_reusing(
    masking: &JwtMasking,
    real: &RealValue,
    reuse: &dyn Fn(usize) -> Option<Sentinel>,
) -> Result<MaskedJwt, JwtMaskError> {
    let ParsedJwt { header, mut payload } = parse(real.expose()).map_err(JwtMaskError::Malformed)?;
    if masking.mask_claims.is_empty() {
        return Ok(MaskedJwt {
            fake: generic_fake(),
            claims: Vec::new(),
        });
    }
    let mut claims: Vec<MaskedClaim> = Vec::new();
    for claim in &masking.mask_claims {
        let Some(Value::String(value)) = payload.get(claim.as_str()) else {
            continue;
        };
        if value.is_empty() || claims.iter().any(|masked| &masked.claim == claim) {
            continue;
        }
        if value.bytes().any(|byte| byte.is_ascii_control()) {
            return Err(JwtMaskError::UnsafeClaimValue(claim.clone()));
        }
        let real = RealValue::new(value.clone());
        claims.push(MaskedClaim {
            claim: claim.clone(),
            sentinel: reuse(claims.len()).unwrap_or_else(|| Sentinel::mint(&real)),
            real,
        });
    }
    if claims.is_empty() {
        return Err(JwtMaskError::NoClaimMatched);
    }
    for masked in &claims {
        payload.insert(
            masked.claim.as_str().to_string(),
            Value::String(masked.sentinel.as_str().to_string()),
        );
    }
    Ok(MaskedJwt {
        fake: assemble(header, &Value::Object(payload)),
        claims,
    })
}
