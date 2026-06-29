use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use data_encoding::BASE32_NOPAD;
use nats_jwt_rs::ClaimType;
use nats_jwt_rs::types::{GenericFields, NatsLimits};
use nats_jwt_rs::user::User;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha512_256};

use super::nats_permission_claims::NatsPermissionClaims;
use super::{
    AccountName, CallerId, ExternalSubject, JwtError, MintedUserJwt, SpiceDbPrincipal, UserJwtClaims, UserJwtSubject,
};
use crate::permissions::IssuedPermissions;
use crate::signing_key_source::{MintingMaterial, SigningKeyHandle, SigningKeySource};

const HEADER_TYPE: &str = "JWT";
const HEADER_ALGORITHM: &str = "ed25519-nkey";

#[derive(Serialize, Deserialize)]
struct NatsJwtHeader {
    #[serde(rename = "typ")]
    header_type: String,
    #[serde(rename = "alg")]
    algorithm: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    kid: Option<String>,
}

#[derive(Serialize)]
struct NatsUserJwtPayload<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    aud: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    exp: Option<i64>,
    iat: u64,
    iss: String,
    jti: String,
    sub: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    nbf: Option<i64>,
    caller_id: &'a str,
    /// Caller-asserted external subject (Stytch/OIDC sub, mTLS DN, etc.).
    /// `sub` above is the NATS user NKey identity for the connection; this
    /// field preserves the input subject so verify can return what was minted.
    ext_sub: &'a str,
    kid: &'a str,
    data: &'a Value,
    nats: Value,
}

#[derive(Deserialize)]
struct VerifiedPayload {
    #[serde(default)]
    aud: Option<String>,
    iss: String,
    #[allow(dead_code)]
    sub: String,
    caller_id: String,
    /// Caller-asserted external subject; preserved separately from `sub` (which
    /// is the NATS user NKey for the connection). Older tokens minted before
    /// this field existed fall back to `data.spicedb_subject`.
    #[serde(default)]
    ext_sub: Option<String>,
    kid: String,
    data: Value,
    nats: VerifiedNatsBlock,
}

#[derive(Deserialize)]
struct VerifiedNatsBlock {
    #[serde(rename = "pub")]
    publish: VerifiedPermission,
    #[serde(rename = "sub")]
    subscribe: VerifiedPermission,
}

#[derive(Deserialize, Default)]
struct VerifiedPermission {
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)]
    deny: Vec<String>,
}

pub(crate) fn mint_nats_user_jwt(
    claims: &UserJwtClaims,
    material: &MintingMaterial,
    user_subject: &UserJwtSubject,
    issued_at: SystemTime,
    ttl: Duration,
) -> Result<MintedUserJwt, JwtError> {
    let iat_secs = secs_since_unix(issued_at)?;
    let ttl_secs_i64 = i64::try_from(ttl.as_secs().max(1)).unwrap_or(i64::MAX);
    let exp_secs = iat_secs.saturating_add(ttl_secs_i64);

    let mut user = User {
        generic_fields: GenericFields {
            claim_type: ClaimType::User,
            version: 2,
            tags: None,
        },
        issuer_account: Some(material.issuer_public()),
        ..Default::default()
    };
    let perm_claims = NatsPermissionClaims::from(&claims.nats_permissions);
    user.permissions.permissions.publish.allow = perm_claims.publish.allow.clone();
    user.permissions.permissions.subscribe.allow = perm_claims.subscribe.allow.clone();
    user.permissions.permissions.publish.deny = perm_claims.publish.deny.clone();
    user.permissions.permissions.subscribe.deny = perm_claims.subscribe.deny.clone();
    user.permissions.limits = Some(nats_jwt_rs::types::Limits {
        nats_limits: Some(NatsLimits::default()),
        user_limits: None,
    });

    let nats_value = serde_json::to_value(&user).map_err(|e| JwtError::Encode(e.to_string()))?;

    // NATS-style JWTs hash the *final* claim body to derive `jti`: the body
    // already has the real `iss` set and `jti` left empty, so plug `iss` in
    // first, then hash, then fill `jti` for the signed payload. Hashing
    // before setting `iss` would yield a `jti` that downstream NATS JWT
    // validators reject even though our local signature check would pass.
    // Reject empty audience up front rather than emitting `aud=""` that
    // verify_with_material would then refuse — a mint that can't verify is
    // worse than a mint that fails loud.
    if claims.aud.as_str().is_empty() {
        return Err(JwtError::Decode("account name must be non-empty".into()));
    }
    let issuer_public = material.issuer_public();
    let payload_template = NatsUserJwtPayload {
        aud: Some(claims.aud.as_str()),
        exp: Some(exp_secs),
        iat: iat_secs as u64,
        iss: issuer_public.clone(),
        jti: String::new(),
        sub: user_subject.as_str(),
        nbf: Some(iat_secs),
        caller_id: claims.caller_id.as_str(),
        ext_sub: claims.sub.as_str(),
        kid: material.version().as_str(),
        data: &claims.data.0,
        nats: nats_value,
    };

    let encoded_claim = serde_json::to_string(&payload_template).map_err(|e| JwtError::Encode(e.to_string()))?;
    let mut hasher = Sha512_256::new();
    hasher.update(encoded_claim.as_bytes());
    let jti = BASE32_NOPAD.encode(&hasher.finalize());

    let payload = NatsUserJwtPayload {
        jti,
        ..payload_template
    };

    let header = NatsJwtHeader {
        header_type: HEADER_TYPE.into(),
        algorithm: HEADER_ALGORITHM.into(),
        kid: Some(material.version().as_str().to_owned()),
    };

    let hdr = encode_segment(&header)?;
    let claims_segment = encode_segment(&payload)?;
    let signing_input = format!("{hdr}.{claims_segment}");
    let sig = material
        .issuer_keypair()
        .sign(signing_input.as_bytes())
        .map_err(|e| JwtError::Encode(e.to_string()))?;
    let signature = URL_SAFE_NO_PAD.encode(sig);
    MintedUserJwt::new(format!("{signing_input}.{signature}"))
}

pub(crate) fn verify_nats_user_jwt(token: &str, handles: &[SigningKeyHandle]) -> Result<UserJwtClaims, JwtError> {
    if handles.is_empty() {
        return Err(JwtError::NoSigningKeyForKid);
    }

    let header_kid = peek_header_kid(token)?;

    if let Some(kid) = header_kid.as_deref()
        && let Some(handle) = handles.iter().find(|h| h.version().as_str() == kid)
    {
        return verify_with_material(token, &handle.minting_material());
    }

    let mut last_err = None;
    for handle in handles {
        match verify_with_material(token, &handle.minting_material()) {
            Ok(claims) => return Ok(claims),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or(JwtError::NoSigningKeyForKid))
}

pub(crate) fn verify_nats_user_jwt_with_source(
    token: &str,
    source: &dyn SigningKeySource,
) -> Result<UserJwtClaims, JwtError> {
    verify_nats_user_jwt(token, &source.accepted())
}

fn verify_with_material(token: &str, material: &MintingMaterial) -> Result<UserJwtClaims, JwtError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(JwtError::Decode("invalid JWT segment count".into()));
    }

    let header: NatsJwtHeader = decode_segment(parts[0])?;
    if header.header_type != HEADER_TYPE || header.algorithm != HEADER_ALGORITHM {
        return Err(JwtError::Decode("unsupported NATS user JWT header".into()));
    }

    let decoded_sig = URL_SAFE_NO_PAD
        .decode(parts[2].as_bytes())
        .map_err(|e| JwtError::Decode(e.to_string()))?;
    let signing_input = &token.as_bytes()[0..token.len() - parts[2].len() - 1];
    material
        .issuer_keypair()
        .verify(signing_input, &decoded_sig)
        .map_err(|e| JwtError::Decode(e.to_string()))?;

    let payload: VerifiedPayload = decode_segment(parts[1])?;
    if payload.iss != material.issuer_public() {
        return Err(JwtError::Decode("issuer does not match signing key".into()));
    }

    let data = SpiceDbPrincipal(payload.data);
    // Prefer the dedicated `ext_sub` claim (preserves the caller-supplied
    // ExternalSubject across mint→verify); fall back to `data.spicedb_subject`
    // for tokens minted before the dedicated field existed.
    let external_sub = match payload.ext_sub {
        Some(s) => ExternalSubject::new(s),
        None => data
            .spicedb_subject()
            .map(|s| ExternalSubject::new(s.as_str()))
            .transpose()?
            .ok_or_else(|| JwtError::Decode("minted user JWT missing ext_sub / spicedb_subject".into())),
    }?;
    let caller_id = CallerId::new(payload.caller_id)?;
    let aud = AccountName::new(
        payload
            .aud
            .filter(|s| !s.is_empty())
            .ok_or_else(|| JwtError::Decode("user JWT missing aud".into()))?,
    );
    let kid = crate::signing_key_source::KeyVersion::new(payload.kid).map_err(|e| JwtError::Decode(e.to_string()))?;

    let nats_permissions = IssuedPermissions {
        publish_allow: payload
            .nats
            .publish
            .allow
            .into_iter()
            .map(crate::permissions::SubjectPattern::new)
            .collect::<Result<_, _>>()
            .map_err(|e| JwtError::Decode(e.to_string()))?,
        subscribe_allow: payload
            .nats
            .subscribe
            .allow
            .into_iter()
            .map(crate::permissions::SubjectPattern::new)
            .collect::<Result<_, _>>()
            .map_err(|e| JwtError::Decode(e.to_string()))?,
    };

    Ok(UserJwtClaims {
        kid,
        sub: external_sub,
        aud,
        data,
        caller_id,
        nats_permissions,
    })
}

fn peek_header_kid(token: &str) -> Result<Option<String>, JwtError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(JwtError::Decode("invalid JWT segment count".into()));
    }
    let header: NatsJwtHeader = decode_segment(parts[0])?;
    if header.algorithm != HEADER_ALGORITHM {
        return Err(JwtError::Decode("unsupported JWT algorithm".into()));
    }
    Ok(header.kid)
}

fn encode_segment<T: Serialize>(input: &T) -> Result<String, JwtError> {
    let encoded = serde_json::to_string(input).map_err(|e| JwtError::Encode(e.to_string()))?;
    Ok(URL_SAFE_NO_PAD.encode(encoded.as_bytes()))
}

fn decode_segment<T: for<'de> Deserialize<'de>>(input: &str) -> Result<T, JwtError> {
    let decoded = URL_SAFE_NO_PAD
        .decode(input.as_bytes())
        .map_err(|e| JwtError::Decode(e.to_string()))?;
    serde_json::from_slice(&decoded).map_err(|e| JwtError::Decode(e.to_string()))
}

fn secs_since_unix(t: SystemTime) -> Result<i64, JwtError> {
    let secs = t.duration_since(UNIX_EPOCH).map_err(JwtError::SystemTime)?.as_secs();
    i64::try_from(secs).map_err(|_| JwtError::IssuedAtOutOfRange)
}

pub fn decode_nats_user_payload(token: &str) -> Result<Value, JwtError> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(JwtError::Decode("invalid JWT segment count".into()));
    }
    decode_segment(parts[1])
}

#[cfg(test)]
mod tests;
