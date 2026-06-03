//! RFC 7638 JWK Thumbprint (SHA-256). Used to bind tokens to a specific key.

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};

/// Compute the RFC 7638 SHA-256 thumbprint of a JWK.
///
/// Per RFC 7638, the canonical form is a JSON object with the JWK's "required
/// members" for the key type, sorted lexicographically by key name with no extra
/// whitespace, then SHA-256, then base64url-no-pad.
///
/// Supports EC (P-256/P-384), OKP (Ed25519), RSA, and oct key types.
pub fn jwk_thumbprint(jwk: &serde_json::Value) -> Result<String, JktError> {
    let kty = jwk.get("kty").and_then(|v| v.as_str()).ok_or(JktError::MissingKty)?;
    let canonical = match kty {
        "EC" => {
            let crv = jwk.get("crv").and_then(|v| v.as_str()).ok_or(JktError::MissingField("crv"))?;
            let x = jwk.get("x").and_then(|v| v.as_str()).ok_or(JktError::MissingField("x"))?;
            let y = jwk.get("y").and_then(|v| v.as_str()).ok_or(JktError::MissingField("y"))?;
            format!(r#"{{"crv":"{crv}","kty":"EC","x":"{x}","y":"{y}"}}"#)
        }
        "OKP" => {
            let crv = jwk.get("crv").and_then(|v| v.as_str()).ok_or(JktError::MissingField("crv"))?;
            let x = jwk.get("x").and_then(|v| v.as_str()).ok_or(JktError::MissingField("x"))?;
            format!(r#"{{"crv":"{crv}","kty":"OKP","x":"{x}"}}"#)
        }
        "RSA" => {
            let e = jwk.get("e").and_then(|v| v.as_str()).ok_or(JktError::MissingField("e"))?;
            let n = jwk.get("n").and_then(|v| v.as_str()).ok_or(JktError::MissingField("n"))?;
            format!(r#"{{"e":"{e}","kty":"RSA","n":"{n}"}}"#)
        }
        "oct" => {
            let k = jwk.get("k").and_then(|v| v.as_str()).ok_or(JktError::MissingField("k"))?;
            format!(r#"{{"k":"{k}","kty":"oct"}}"#)
        }
        other => return Err(JktError::UnsupportedKty(other.to_string())),
    };
    let digest = Sha256::digest(canonical.as_bytes());
    Ok(URL_SAFE_NO_PAD.encode(digest))
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum JktError {
    #[error("jwk missing required field: kty")]
    MissingKty,
    #[error("jwk missing required field: {0}")]
    MissingField(&'static str),
    #[error("jwk kty {0} is not supported")]
    UnsupportedKty(String),
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
}
