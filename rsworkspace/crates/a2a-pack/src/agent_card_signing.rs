//! AgentCard JWS signing and verification per A2A spec.
//!
//! Implements signed AgentCards using JWS JSON General Serialization where
//! each entry in `signatures` has fields `protected`, `signature`, and an
//! optional `header`. Signing input is `BASE64URL(protected) || '.' ||
//! BASE64URL(payload)` where `payload` is the canonical JSON serialization of
//! the AgentCard **with the `signatures` field omitted**.
//!
//! The implementation works on `serde_json::Value` so it composes with both
//! the prost-generated `a2a_types::AgentCard` (after `serde_json::to_value`)
//! and any other JSON-shaped card.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One JWS signature entry in the AgentCard's `signatures` array.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentCardSignature {
    /// `BASE64URL` of the protected JOSE header JSON.
    pub protected: String,
    /// `BASE64URL` of the signature bytes.
    pub signature: String,
    /// Unprotected header (not covered by the signature).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<Value>,
}

/// Errors that occur while signing or verifying an AgentCard.
#[derive(Debug)]
pub enum AgentCardSignError {
    NotAnObject,
    Serialize(serde_json::Error),
    Encode(jsonwebtoken::errors::Error),
    Decode(jsonwebtoken::errors::Error),
    InvalidProtectedHeader(String),
    InvalidSignaturesField,
    MissingSignatures,
    NoMatchingKey,
    Base64(base64::DecodeError),
}

impl fmt::Display for AgentCardSignError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAnObject => f.write_str("AgentCard must be a JSON object"),
            Self::Serialize(e) => write!(f, "failed to serialize signing payload: {e}"),
            Self::Encode(e) => write!(f, "JWS encode failed: {e}"),
            Self::Decode(e) => write!(f, "JWS decode failed: {e}"),
            Self::InvalidProtectedHeader(msg) => write!(f, "invalid JWS protected header: {msg}"),
            Self::InvalidSignaturesField => f.write_str("`signatures` field on AgentCard must be an array"),
            Self::MissingSignatures => f.write_str("AgentCard has no signatures to verify"),
            Self::NoMatchingKey => f.write_str("no key matched any signature `kid`"),
            Self::Base64(e) => write!(f, "base64 decode failed: {e}"),
        }
    }
}

impl std::error::Error for AgentCardSignError {}

/// Inputs required to add a signature to an AgentCard.
#[derive(Clone)]
pub struct AgentCardSigner<'a> {
    pub algorithm: Algorithm,
    pub key: &'a EncodingKey,
    /// Optional `kid` placed in the protected header so verifiers can pick
    /// the matching public key.
    pub kid: Option<String>,
    /// Optional extra fields to merge into the protected JOSE header.
    pub additional_protected: Option<Value>,
}

/// Sign an AgentCard JSON value and return a clone of the card with the new
/// [`AgentCardSignature`] appended to `signatures`.
pub fn sign_agent_card(card: &Value, signer: &AgentCardSigner<'_>) -> Result<Value, AgentCardSignError> {
    if !card.is_object() {
        return Err(AgentCardSignError::NotAnObject);
    }

    let payload_value = strip_signatures(card);
    let payload_canonical = canonical_json(&payload_value).map_err(AgentCardSignError::Serialize)?;
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload_canonical.as_bytes());

    let mut protected = Map::new();
    protected.insert("alg".to_string(), Value::String(algorithm_name(signer.algorithm).to_string()));
    if let Some(kid) = signer.kid.as_ref() {
        protected.insert("kid".to_string(), Value::String(kid.clone()));
    }
    if let Some(extra) = signer.additional_protected.as_ref()
        && let Some(extra_map) = extra.as_object()
    {
        for (k, v) in extra_map {
            protected.insert(k.clone(), v.clone());
        }
    }
    let protected_value = Value::Object(protected);
    let protected_canonical = canonical_json(&protected_value).map_err(AgentCardSignError::Serialize)?;
    let protected_b64 = URL_SAFE_NO_PAD.encode(protected_canonical.as_bytes());

    let signing_input = format!("{protected_b64}.{payload_b64}");
    let signature = jsonwebtoken::crypto::sign(signing_input.as_bytes(), signer.key, signer.algorithm)
        .map_err(AgentCardSignError::Encode)?;

    let entry = AgentCardSignature {
        protected: protected_b64,
        signature,
        header: None,
    };

    let mut signed = card.clone();
    let arr = signed
        .as_object_mut()
        .expect("checked above")
        .entry("signatures")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or(AgentCardSignError::InvalidSignaturesField)?;
    arr.push(serde_json::to_value(entry).map_err(AgentCardSignError::Serialize)?);
    Ok(signed)
}

/// Resolve a verification key for the given protected JOSE header.
///
/// Implementors typically inspect `header["kid"]` (or `header["jwk"]` if
/// embedded) and return the matching `DecodingKey`.
pub trait AgentCardKeyResolver {
    fn resolve(&self, protected_header: &Value) -> Option<(Algorithm, DecodingKey)>;
}

impl<F> AgentCardKeyResolver for F
where
    F: Fn(&Value) -> Option<(Algorithm, DecodingKey)>,
{
    fn resolve(&self, protected_header: &Value) -> Option<(Algorithm, DecodingKey)> {
        (self)(protected_header)
    }
}

/// Verify at least one signature on an AgentCard. Returns the protected header
/// of the first signature that verified.
pub fn verify_agent_card<R>(card: &Value, resolver: &R) -> Result<Value, AgentCardSignError>
where
    R: AgentCardKeyResolver,
{
    if !card.is_object() {
        return Err(AgentCardSignError::NotAnObject);
    }
    let signatures = card
        .get("signatures")
        .ok_or(AgentCardSignError::MissingSignatures)?
        .as_array()
        .ok_or(AgentCardSignError::InvalidSignaturesField)?;
    if signatures.is_empty() {
        return Err(AgentCardSignError::MissingSignatures);
    }

    let payload_value = strip_signatures(card);
    let payload_canonical = canonical_json(&payload_value).map_err(AgentCardSignError::Serialize)?;
    let payload_b64 = URL_SAFE_NO_PAD.encode(payload_canonical.as_bytes());

    let mut matched_key = false;
    for sig_value in signatures {
        let sig: AgentCardSignature =
            serde_json::from_value(sig_value.clone()).map_err(AgentCardSignError::Serialize)?;
        let protected_bytes = URL_SAFE_NO_PAD
            .decode(sig.protected.as_bytes())
            .map_err(AgentCardSignError::Base64)?;
        let protected_header: Value =
            serde_json::from_slice(&protected_bytes).map_err(AgentCardSignError::Serialize)?;

        let Some((alg, key)) = resolver.resolve(&protected_header) else {
            continue;
        };
        matched_key = true;

        let signing_input = format!("{}.{}", sig.protected, payload_b64);
        let valid = jsonwebtoken::crypto::verify(&sig.signature, signing_input.as_bytes(), &key, alg)
            .map_err(AgentCardSignError::Decode)?;
        if valid {
            return Ok(protected_header);
        }
    }
    if matched_key {
        Err(AgentCardSignError::Decode(
            jsonwebtoken::errors::ErrorKind::InvalidSignature.into(),
        ))
    } else {
        Err(AgentCardSignError::NoMatchingKey)
    }
}

fn strip_signatures(card: &Value) -> Value {
    let mut clone = card.clone();
    if let Some(map) = clone.as_object_mut() {
        map.remove("signatures");
    }
    clone
}

fn canonical_json(value: &Value) -> Result<String, serde_json::Error> {
    let mut buf = Vec::new();
    write_canonical(value, &mut buf)?;
    Ok(String::from_utf8(buf).expect("canonical json is ascii-safe utf-8"))
}

fn write_canonical(value: &Value, out: &mut Vec<u8>) -> Result<(), serde_json::Error> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            let s = serde_json::to_string(value)?;
            out.extend_from_slice(s.as_bytes());
        }
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(item, out)?;
            }
            out.push(b']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push(b'{');
            for (i, k) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                let key_json = serde_json::to_string(k)?;
                out.extend_from_slice(key_json.as_bytes());
                out.push(b':');
                write_canonical(&map[*k], out)?;
            }
            out.push(b'}');
        }
    }
    Ok(())
}

fn algorithm_name(alg: Algorithm) -> &'static str {
    match alg {
        Algorithm::HS256 => "HS256",
        Algorithm::HS384 => "HS384",
        Algorithm::HS512 => "HS512",
        Algorithm::ES256 => "ES256",
        Algorithm::ES384 => "ES384",
        Algorithm::RS256 => "RS256",
        Algorithm::RS384 => "RS384",
        Algorithm::RS512 => "RS512",
        Algorithm::PS256 => "PS256",
        Algorithm::PS384 => "PS384",
        Algorithm::PS512 => "PS512",
        Algorithm::EdDSA => "EdDSA",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hmac_signer<'a>(key: &'a EncodingKey) -> AgentCardSigner<'a> {
        AgentCardSigner {
            algorithm: Algorithm::HS256,
            key,
            kid: Some("test-key".into()),
            additional_protected: None,
        }
    }

    fn card() -> Value {
        serde_json::json!({
            "name": "demo-agent",
            "version": "0.1.0",
            "supportedInterfaces": [{
                "url": "https://example.com/a2a",
                "protocolBinding": "JSONRPC",
                "protocolVersion": "0.3.0"
            }]
        })
    }

    #[test]
    fn sign_then_verify_round_trip() {
        let secret = b"a-very-secret-key-only-for-tests";
        let enc = EncodingKey::from_secret(secret);
        let dec = DecodingKey::from_secret(secret);
        let signed = sign_agent_card(&card(), &hmac_signer(&enc)).expect("sign");
        assert!(signed.get("signatures").and_then(|v| v.as_array()).is_some());
        let resolver = |_h: &Value| Some((Algorithm::HS256, dec.clone()));
        let header = verify_agent_card(&signed, &resolver).expect("verify");
        assert_eq!(header["alg"], "HS256");
        assert_eq!(header["kid"], "test-key");
    }

    #[test]
    fn payload_tamper_invalidates_signature() {
        let secret = b"a-very-secret-key-only-for-tests";
        let enc = EncodingKey::from_secret(secret);
        let dec = DecodingKey::from_secret(secret);
        let mut signed = sign_agent_card(&card(), &hmac_signer(&enc)).expect("sign");
        signed["name"] = serde_json::json!("evil-agent");
        let resolver = |_h: &Value| Some((Algorithm::HS256, dec.clone()));
        let err = verify_agent_card(&signed, &resolver).expect_err("must fail");
        matches!(err, AgentCardSignError::Decode(_));
    }

    #[test]
    fn signature_field_not_covered_by_signing() {
        let secret = b"a-very-secret-key-only-for-tests";
        let enc = EncodingKey::from_secret(secret);
        let dec = DecodingKey::from_secret(secret);
        let signed = sign_agent_card(&card(), &hmac_signer(&enc)).expect("sign");

        let mut twice_signed = signed.clone();
        let arr = twice_signed
            .get_mut("signatures")
            .unwrap()
            .as_array_mut()
            .unwrap();
        arr.push(serde_json::json!({"protected": "garbage", "signature": "junk"}));
        let resolver = |_h: &Value| Some((Algorithm::HS256, dec.clone()));
        verify_agent_card(&twice_signed, &resolver).expect("first signature still verifies");
    }

    #[test]
    fn rejects_non_object_card() {
        let secret = b"k";
        let enc = EncodingKey::from_secret(secret);
        let err = sign_agent_card(&Value::String("not a card".into()), &hmac_signer(&enc)).unwrap_err();
        assert!(matches!(err, AgentCardSignError::NotAnObject));
    }

    #[test]
    fn verify_with_no_signatures_errors() {
        let resolver = |_h: &Value| None::<(Algorithm, DecodingKey)>;
        let err = verify_agent_card(&card(), &resolver).unwrap_err();
        assert!(matches!(err, AgentCardSignError::MissingSignatures));
    }

    #[test]
    fn verify_with_unmatched_kid_returns_no_matching_key() {
        let secret = b"a-very-secret-key-only-for-tests";
        let enc = EncodingKey::from_secret(secret);
        let signed = sign_agent_card(&card(), &hmac_signer(&enc)).expect("sign");
        let resolver = |_h: &Value| None::<(Algorithm, DecodingKey)>;
        let err = verify_agent_card(&signed, &resolver).unwrap_err();
        assert!(matches!(err, AgentCardSignError::NoMatchingKey));
    }

    #[test]
    fn canonical_json_sorts_object_keys() {
        let v = serde_json::json!({"b": 1, "a": 2, "c": {"y": 1, "x": 2}});
        let s = canonical_json(&v).unwrap();
        assert_eq!(s, r#"{"a":2,"b":1,"c":{"x":2,"y":1}}"#);
    }
}
