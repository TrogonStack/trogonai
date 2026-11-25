use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde_json::{Map, Value};

use crate::JwtSvidError;
use crate::constants::MAX_JWT_SVID_BYTES;
use crate::jwt_algorithm::JwtSvidAlgorithm;

/// A syntactically valid compact JWS whose header names an allowed algorithm.
/// Nothing in it is trusted yet: the signature has not been checked.
pub(crate) struct ParsedToken<'a> {
    pub(crate) algorithm: JwtSvidAlgorithm,
    pub(crate) key_id: Option<String>,
    pub(crate) claims: Map<String, Value>,
    /// `base64url(header) "." base64url(payload)`, the signed bytes.
    pub(crate) signing_input: &'a str,
    pub(crate) signature: &'a str,
}

impl<'a> ParsedToken<'a> {
    pub(crate) fn parse(token: &'a str) -> Result<Self, JwtSvidError> {
        if token.len() > MAX_JWT_SVID_BYTES {
            return Err(JwtSvidError::TokenTooLarge);
        }
        if token.trim_start().starts_with('{') {
            return Err(JwtSvidError::JsonSerialization);
        }
        let mut segments = token.split('.');
        let (Some(header), Some(payload), Some(signature), None) =
            (segments.next(), segments.next(), segments.next(), segments.next())
        else {
            return Err(JwtSvidError::NotCompactSerialization);
        };
        if header.is_empty() || payload.is_empty() {
            return Err(JwtSvidError::NotCompactSerialization);
        }
        let header = object(header, JwtSvidError::MalformedHeader)?;
        let (algorithm, key_id) = header_fields(&header)?;
        // The signature segment is checked after `alg`, so an unsigned
        // `alg: none` token is refused for its algorithm.
        if signature.is_empty() || !is_base64url(signature) || URL_SAFE_NO_PAD.decode(signature).is_err() {
            return Err(JwtSvidError::MalformedSegment);
        }
        let claims = object(payload, JwtSvidError::MalformedClaims)?;
        Ok(Self {
            algorithm,
            key_id,
            claims,
            signing_input: &token[..token.len() - signature.len() - 1],
            signature,
        })
    }
}

fn is_base64url(segment: &str) -> bool {
    segment
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn object(segment: &str, error: JwtSvidError) -> Result<Map<String, Value>, JwtSvidError> {
    if !is_base64url(segment) {
        return Err(JwtSvidError::MalformedSegment);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(segment)
        .map_err(|_| JwtSvidError::MalformedSegment)?;
    match serde_json::from_slice(&bytes) {
        Ok(Value::Object(map)) => Ok(map),
        _ => Err(error),
    }
}

fn header_fields(header: &Map<String, Value>) -> Result<(JwtSvidAlgorithm, Option<String>), JwtSvidError> {
    let algorithm = match header.get("alg") {
        None => return Err(JwtSvidError::MissingAlgorithm),
        Some(Value::String(name)) => JwtSvidAlgorithm::parse(name).ok_or(JwtSvidError::AlgorithmNotAllowed)?,
        Some(_) => return Err(JwtSvidError::AlgorithmNotAllowed),
    };
    if header
        .keys()
        .any(|name| !matches!(name.as_str(), "alg" | "typ" | "kid"))
    {
        return Err(JwtSvidError::UnexpectedHeader);
    }
    match header.get("typ") {
        None => {}
        Some(Value::String(typ)) if typ == "JWT" || typ == "JOSE" => {}
        Some(_) => return Err(JwtSvidError::InvalidType),
    }
    let key_id = match header.get("kid") {
        None => None,
        Some(Value::String(kid)) if !kid.is_empty() => Some(kid.clone()),
        Some(_) => return Err(JwtSvidError::MalformedHeader),
    };
    Ok((algorithm, key_id))
}
