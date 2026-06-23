//! RFC 7638 JWK Thumbprint (SHA-256). Used to bind tokens to a specific key.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

/// Compute the RFC 7638 SHA-256 thumbprint of a JWK.
///
/// Per RFC 7638, the canonical form is a JSON object with the JWK's "required
/// members" for the key type, sorted lexicographically by key name with no
/// extra whitespace. Building the JSON via `serde_json` keeps the string
/// values properly escaped — interpolating raw `format!` strings would
/// produce a non-canonical digest for values containing quotes or
/// backslashes, weakening the `agent_jkt` binding for crafted `cnf.jwk`
/// payloads.
///
/// Supports EC (P-256/P-384), OKP (Ed25519), RSA, and oct key types.
pub fn jwk_thumbprint(jwk: &serde_json::Value) -> Result<String, JktError> {
    let kty = jwk.get("kty").and_then(|v| v.as_str()).ok_or(JktError::MissingKty)?;
    let mut required = Map::new();
    match kty {
        "EC" => {
            require_str(jwk, "crv", &mut required)?;
            required.insert("kty".into(), Value::String("EC".into()));
            require_str(jwk, "x", &mut required)?;
            require_str(jwk, "y", &mut required)?;
        }
        "OKP" => {
            require_str(jwk, "crv", &mut required)?;
            required.insert("kty".into(), Value::String("OKP".into()));
            require_str(jwk, "x", &mut required)?;
        }
        "RSA" => {
            require_str(jwk, "e", &mut required)?;
            required.insert("kty".into(), Value::String("RSA".into()));
            require_str(jwk, "n", &mut required)?;
        }
        "oct" => {
            require_str(jwk, "k", &mut required)?;
            required.insert("kty".into(), Value::String("oct".into()));
        }
        other => return Err(JktError::UnsupportedKty(other.to_string())),
    }
    // serde_json::Map (with the `preserve_order` feature off) is a BTreeMap,
    // which already iterates lexicographically. Render with the default
    // compact serializer — RFC 7638 requires no whitespace between members.
    let canonical = serde_json::to_string(&Value::Object(required)).map_err(JktError::Serialize)?;
    let digest = Sha256::digest(canonical.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(digest))
}

fn require_str(jwk: &Value, field: &'static str, out: &mut Map<String, Value>) -> Result<(), JktError> {
    let value = jwk
        .get(field)
        .and_then(|v| v.as_str())
        .ok_or(JktError::MissingField(field))?;
    out.insert(field.to_owned(), Value::String(value.to_owned()));
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum JktError {
    #[error("jwk missing required field: kty")]
    MissingKty,
    #[error("jwk missing required field: {0}")]
    MissingField(&'static str),
    #[error("jwk kty {0} is not supported")]
    UnsupportedKty(String),
    #[error("canonical JWK could not be serialized: {0}")]
    Serialize(#[source] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ec_p256_thumbprint_matches_rfc7638_example_shape() {
        // Inputs are stable strings; we don't assert the actual RFC example value
        // here, but we verify determinism and base64url-no-pad encoding shape.
        let jwk = serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU",
            "y": "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0",
            "use": "sig",
            "kid": "ignored"
        });
        let a = jwk_thumbprint(&jwk).unwrap();
        let b = jwk_thumbprint(&jwk).unwrap();
        assert_eq!(a, b);
        assert!(!a.contains('='));
    }

    #[test]
    fn rejects_unknown_kty() {
        let jwk = serde_json::json!({"kty": "WAT"});
        let err = jwk_thumbprint(&jwk).unwrap_err();
        assert!(matches!(err, JktError::UnsupportedKty(_)));
    }

    #[test]
    fn values_with_quotes_or_backslashes_are_escaped_in_canonical_form() {
        // Crafted (invalid base64url, but the JKT helper takes whatever serde
        // hands it) `x` containing characters that need JSON escaping. The old
        // format!-based canonicalizer produced literal embedded quotes, which
        // both broke the JSON shape and made the digest match a different
        // canonical string than a compliant implementation. Verify the
        // canonical-form path escapes them.
        let jwk = serde_json::json!({
            "kty": "EC",
            "crv": "P-256",
            "x": "ab\"cd",
            "y": "ef\\gh",
        });
        let digest = jwk_thumbprint(&jwk).expect("thumbprint succeeds with escapable chars");
        // Smoke: ensure it's the same as feeding the SAME escaped canonical
        // form through SHA-256 directly. If the helper ever regresses to raw
        // `format!` interpolation, the bytes hashed change and this fails.
        let expected_canonical = r#"{"crv":"P-256","kty":"EC","x":"ab\"cd","y":"ef\\gh"}"#;
        let expected = URL_SAFE_NO_PAD.encode(Sha256::digest(expected_canonical.as_bytes()));
        assert_eq!(digest, expected);
    }
}
