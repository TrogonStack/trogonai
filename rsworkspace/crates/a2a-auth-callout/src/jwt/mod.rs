mod nats_permission_claims;
mod nats_user_jwt;
mod user_jwt_subject;

use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

pub use nats_permission_claims::{NatsPermissionClaims, NatsSubjectPermission};
pub use nats_user_jwt::decode_nats_user_payload;
pub use user_jwt_subject::UserJwtSubject;

use crate::error::AuthCalloutError;
use crate::permissions::IssuedPermissions;
use crate::signing_key_source::{KeyVersion, MintingMaterial, SigningKeyHandle, SigningKeySource};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountName(String);

impl AccountName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AccountName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub type AudienceAccount = AccountName;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExternalSubject(String);

impl ExternalSubject {
    pub fn new(subject: impl Into<String>) -> Result<Self, JwtError> {
        let s = subject.into();
        if s.is_empty() {
            return Err(JwtError::InvalidExternalSubject);
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CallerId(String);

impl CallerId {
    pub fn new(segment: impl Into<String>) -> Result<Self, JwtError> {
        let s = segment.into();
        validate_caller_segment(&s).map(|()| Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn validate_caller_segment(s: &str) -> Result<(), JwtError> {
    if s.is_empty() || s.contains('.') {
        return Err(JwtError::InvalidCallerId);
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpiceDbSubject(String);

impl SpiceDbSubject {
    pub fn new(subject: impl Into<String>) -> Self {
        Self(subject.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SpiceDbPrincipal(pub Value);

impl SpiceDbPrincipal {
    pub fn new(subject: impl Into<String>) -> Self {
        Self(json!({ "spicedb_subject": subject.into() }))
    }

    pub fn spicedb_subject(&self) -> Option<SpiceDbSubject> {
        self.0
            .get("spicedb_subject")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(SpiceDbSubject::new)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintedUserJwt(String);

impl MintedUserJwt {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }

    pub fn ensure_fresh(&self) -> Result<(), JwtError> {
        let payload = decode_nats_user_payload(self.as_str())?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(JwtError::SystemTime)?
            .as_secs();
        let now_i64 = i64::try_from(now).map_err(|_| JwtError::IssuedAtOutOfRange)?;
        let exp = payload
            .get("exp")
            .and_then(Value::as_i64)
            .ok_or_else(|| JwtError::Decode("user JWT missing exp".into()))?;
        if exp <= now_i64 {
            return Err(JwtError::Decode("user JWT expired".into()));
        }
        if let Some(nbf) = payload.get("nbf").and_then(Value::as_i64)
            && nbf > now_i64
        {
            return Err(JwtError::Decode("user JWT not yet valid".into()));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct SigningKey(nkeys::KeyPair);

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SigningKey")
    }
}

impl SigningKey {
    pub fn from_seed(seed: impl AsRef<str>) -> Result<Self, JwtError> {
        nkeys::KeyPair::from_seed(seed.as_ref())
            .map(Self)
            .map_err(|e| JwtError::InvalidSigningSeed(e.to_string()))
    }

    pub(crate) fn keypair(&self) -> &nkeys::KeyPair {
        &self.0
    }
}

#[derive(Debug)]
pub enum JwtError {
    Encode(String),
    Decode(String),
    SystemTime(std::time::SystemTimeError),
    InvalidCallerId,
    InvalidExternalSubject,
    IssuedAtOutOfRange,
    NoSigningKeyForKid,
    InvalidSigningSeed(String),
}

impl fmt::Display for JwtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Encode(e) => write!(f, "JWT encode error: {e}"),
            Self::Decode(e) => write!(f, "JWT decode error: {e}"),
            Self::SystemTime(e) => write!(f, "system time error: {e}"),
            Self::InvalidCallerId => f.write_str("caller_id invalid for NATS subject token"),
            Self::InvalidExternalSubject => f.write_str("external subject must be non-empty"),
            Self::IssuedAtOutOfRange => f.write_str("issued-at timestamp out of portable range"),
            Self::NoSigningKeyForKid => f.write_str("no accepted signing key matched token kid"),
            Self::InvalidSigningSeed(e) => write!(f, "invalid account signing seed: {e}"),
        }
    }
}

impl std::error::Error for JwtError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::SystemTime(e) => Some(e),
            _ => None,
        }
    }
}

impl From<JwtError> for AuthCalloutError {
    fn from(value: JwtError) -> Self {
        Self::JwtMint(value.to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserJwtClaims {
    pub kid: KeyVersion,
    pub sub: ExternalSubject,
    pub aud: AccountName,
    pub data: SpiceDbPrincipal,
    pub caller_id: CallerId,
    pub nats_permissions: IssuedPermissions,
}

impl UserJwtClaims {
    pub fn mint(
        &self,
        material: &MintingMaterial,
        user_subject: &UserJwtSubject,
        issued_at: SystemTime,
        ttl: Duration,
    ) -> Result<MintedUserJwt, JwtError> {
        nats_user_jwt::mint_nats_user_jwt(self, material, user_subject, issued_at, ttl)
    }

    pub fn verify_with_source(token: &str, source: &dyn SigningKeySource) -> Result<Self, JwtError> {
        nats_user_jwt::verify_nats_user_jwt_with_source(token, source)
    }

    pub fn verify_with_handles(token: &str, handles: &[SigningKeyHandle]) -> Result<Self, JwtError> {
        nats_user_jwt::verify_nats_user_jwt(token, handles)
    }

    pub fn verify_minted_user_jwt(
        token: &str,
        source: &dyn SigningKeySource,
        expected_aud: &AccountName,
    ) -> Result<Self, JwtError> {
        let claims = Self::verify_with_source(token, source)?;
        if claims.aud.as_str() != expected_aud.as_str() {
            return Err(JwtError::Decode("user JWT audience does not match gateway account".into()));
        }
        let payload = decode_nats_user_payload(token)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(JwtError::SystemTime)?
            .as_secs();
        let now_i64 = i64::try_from(now).map_err(|_| JwtError::IssuedAtOutOfRange)?;
        let exp = payload
            .get("exp")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| JwtError::Decode("user JWT missing exp".into()))?;
        if exp <= now_i64 {
            return Err(JwtError::Decode("user JWT expired".into()));
        }
        if let Some(nbf) = payload.get("nbf").and_then(serde_json::Value::as_i64)
            && nbf > now_i64
        {
            return Err(JwtError::Decode("user JWT not yet valid".into()));
        }
        Ok(claims)
    }

    #[cfg(test)]
    fn mint_for_test_ttl(
        &self,
        material: &MintingMaterial,
        user_subject: &UserJwtSubject,
        ttl: Duration,
    ) -> Result<MintedUserJwt, JwtError> {
        self.mint(
            material,
            user_subject,
            std::time::UNIX_EPOCH + Duration::from_secs(1_000),
            ttl,
        )
    }
}

pub fn caller_id_from_minted_jwt(token: &str) -> Result<CallerId, JwtError> {
    let payload: serde_json::Value = nats_user_jwt::decode_nats_user_payload(token)?;
    let caller_id = payload
        .get("caller_id")
        .and_then(Value::as_str)
        .ok_or_else(|| JwtError::Decode("minted user JWT missing caller_id".into()))?;
    CallerId::new(caller_id)
}

pub(crate) fn derive_caller_id(external_sub: &str, tenant: &AccountName) -> Result<CallerId, JwtError> {
    let mut hasher = Sha256::new();
    hasher.update(external_sub.as_bytes());
    hasher.update(b"|");
    hasher.update(tenant.as_str().as_bytes());
    let digest = hasher.finalize();
    CallerId::new(hex::encode(&digest[..16]))
}

pub(crate) fn spicedb_bundle_for_opaque(principal_hint: impl Into<Value>) -> SpiceDbPrincipal {
    SpiceDbPrincipal(principal_hint.into())
}

pub(crate) fn spicedb_principal_from_oidc_claims(claims: &Value) -> SpiceDbPrincipal {
    if let Some(p) = claims.get("spicedb_principal") {
        SpiceDbPrincipal(p.clone())
    } else if let Some(sub) = claims.get("sub") {
        SpiceDbPrincipal(json!({ "spicedb_subject": sub }))
    } else {
        SpiceDbPrincipal(json!({}))
    }
}

pub(crate) fn external_subject_from_der(prefix: &str, cert_der: &[u8]) -> Result<ExternalSubject, JwtError> {
    let mut hasher = Sha256::new();
    hasher.update(cert_der);
    ExternalSubject::new(format!("{}|{}", prefix, hex::encode(hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signing_key_source::{KeyVersion, SigningKeyHandle};
    use nkeys::KeyPair;
    use serde_json::json;

    #[test]
    fn mint_decodes_expected_claims() {
        let issuer = KeyPair::new_account();
        let issuer_seed = issuer.seed().expect("issuer seed");
        let user = KeyPair::new_user();
        let material = MintingMaterial::new(
            SigningKey::from_seed(&issuer_seed).unwrap().keypair().clone(),
            KeyVersion::new("test").unwrap(),
        );
        let caller_id = CallerId::new("caller1").unwrap();
        let claims = UserJwtClaims {
            kid: material.version().clone(),
            sub: ExternalSubject::new("alice").unwrap(),
            aud: AccountName::new("tenant-acme"),
            data: SpiceDbPrincipal(json!({"spicedb_subject": "user/alice"})),
            nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
            caller_id,
        };
        let subject =
            UserJwtSubject::from_user_nkey(crate::wire::NkeyPublic::parse(user.public_key()).unwrap());
        let token = claims
            .mint_for_test_ttl(&material, &subject, Duration::from_secs(60))
            .unwrap();
        let handle = SigningKeyHandle::new(
            material.version().clone(),
            SigningKey::from_seed(&issuer_seed).unwrap(),
        );
        let decoded = UserJwtClaims::verify_with_handles(token.as_str(), &[handle]).unwrap();
        assert_eq!(decoded.sub.as_str(), "user/alice");
        assert_eq!(decoded.aud.as_str(), "tenant-acme");
        assert_eq!(decoded.caller_id.as_str(), "caller1");
        assert_eq!(decoded.data.spicedb_subject().unwrap().as_str(), "user/alice");
    }

    #[test]
    fn mint_rejects_wrong_verification_key() {
        let issuer_a = KeyPair::new_account();
        let issuer_a_seed = issuer_a.seed().expect("issuer seed");
        let issuer_b = KeyPair::new_account();
        let issuer_b_seed = issuer_b.seed().expect("issuer b seed");
        let user = KeyPair::new_user();
        let material = MintingMaterial::new(
            SigningKey::from_seed(&issuer_a_seed).unwrap().keypair().clone(),
            KeyVersion::new("test").unwrap(),
        );
        let caller_id = CallerId::new("cid").unwrap();
        let claims = UserJwtClaims {
            kid: material.version().clone(),
            sub: ExternalSubject::new("s").unwrap(),
            aud: AudienceAccount::new("a"),
            data: SpiceDbPrincipal(json!({})),
            nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
            caller_id,
        };
        let subject =
            UserJwtSubject::from_user_nkey(crate::wire::NkeyPublic::parse(user.public_key()).unwrap());
        let token = claims
            .mint_for_test_ttl(&material, &subject, Duration::from_secs(60))
            .unwrap();
        let wrong = SigningKeyHandle::new(
            KeyVersion::new("test").unwrap(),
            SigningKey::from_seed(&issuer_b_seed).unwrap(),
        );
        assert!(UserJwtClaims::verify_with_handles(token.as_str(), &[wrong]).is_err());
    }

    #[test]
    fn caller_id_rejects_dots() {
        assert!(CallerId::new("a.b").unwrap_err().to_string().contains("caller_id"));
    }

    #[test]
    fn external_subject_requires_non_empty() {
        assert!(ExternalSubject::new("").unwrap_err().to_string().contains("external subject"));
    }

    #[test]
    fn spicedb_principal_prefers_custom_claim() {
        let v = json!({ "sub": "x", "spicedb_principal": { "kind": "special" } });
        let p = spicedb_principal_from_oidc_claims(&v);
        assert_eq!(p.0["kind"], "special");
    }

    #[test]
    fn spicedb_subject_accessor_reads_claim() {
        let p = SpiceDbPrincipal::new("user/alice");
        assert_eq!(p.spicedb_subject().unwrap().as_str(), "user/alice");
    }

    #[test]
    fn spicedb_subject_accessor_absent_when_missing_or_empty() {
        assert!(SpiceDbPrincipal(json!({})).spicedb_subject().is_none());
        assert!(SpiceDbPrincipal(json!({"spicedb_subject": ""})).spicedb_subject().is_none());
    }
}
