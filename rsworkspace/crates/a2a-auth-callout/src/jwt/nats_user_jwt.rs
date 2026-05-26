use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use data_encoding::BASE32_NOPAD;
use nats_jwt_rs::types::{GenericFields, NatsLimits};
use nats_jwt_rs::user::User;
use nats_jwt_rs::ClaimType;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha512_256};

use super::{
    AccountName, CallerId, ExternalSubject, JwtError, MintedUserJwt, SpiceDbPrincipal, UserJwtClaims,
    UserJwtSubject,
};
use super::nats_permission_claims::NatsPermissionClaims;
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

    let payload_template = NatsUserJwtPayload {
        aud: Some(claims.aud.as_str()),
        exp: Some(exp_secs),
        iat: iat_secs as u64,
        iss: String::new(),
        jti: String::new(),
        sub: user_subject.as_str(),
        nbf: Some(iat_secs),
        caller_id: claims.caller_id.as_str(),
        kid: material.version().as_str(),
        data: &claims.data.0,
        nats: nats_value,
    };

    let encoded_claim = serde_json::to_string(&payload_template).map_err(|e| JwtError::Encode(e.to_string()))?;
    let mut hasher = Sha512_256::new();
    hasher.update(encoded_claim.as_bytes());
    let jti = BASE32_NOPAD.encode(&hasher.finalize());

    let payload = NatsUserJwtPayload {
        iss: material.issuer_public(),
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
    Ok(MintedUserJwt::new(format!("{signing_input}.{signature}")))
}

pub(crate) fn verify_nats_user_jwt(
    token: &str,
    handles: &[SigningKeyHandle],
) -> Result<UserJwtClaims, JwtError> {
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
    let external_sub = data
        .spicedb_subject()
        .map(|s| ExternalSubject::new(s.as_str()))
        .transpose()
        .map_err(|e| JwtError::Decode(e.to_string()))?
        .ok_or_else(|| {
            JwtError::Decode("minted user JWT data missing spicedb_subject".into())
        })?;
    let caller_id = CallerId::new(payload.caller_id)?;
    let aud = AccountName::new(
        payload
            .aud
            .filter(|s| !s.is_empty())
            .ok_or_else(|| JwtError::Decode("user JWT missing aud".into()))?,
    );
    let kid = crate::signing_key_source::KeyVersion::new(payload.kid)
        .map_err(|e| JwtError::Decode(e.to_string()))?;

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
    let secs = t
        .duration_since(UNIX_EPOCH)
        .map_err(JwtError::SystemTime)?
        .as_secs();
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
mod tests {
    use super::*;
    use crate::permissions::IssuedPermissions;
    use crate::signing_key_source::KeyVersion;
    use nkeys::KeyPair;
    use serde_json::json;

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
        let subject = UserJwtSubject::from_user_nkey(
            crate::wire::NkeyPublic::parse(user.public_key()).unwrap(),
        );
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

        let handle = SigningKeyHandle::new(
            material.version().clone(),
            super::super::SigningKey::from_seed(&issuer_seed).unwrap(),
        );
        let verified = verify_with_material(token.as_str(), &handle.minting_material()).unwrap();
        assert_eq!(verified.caller_id.as_str(), "caller1");
        assert_eq!(verified.aud.as_str(), "tenant-acme");
    }
}
