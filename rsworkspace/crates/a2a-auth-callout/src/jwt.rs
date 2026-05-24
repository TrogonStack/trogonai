use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest as _, Sha256};

use crate::error::AuthCalloutError;
use crate::permissions::IssuedPermissions;

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

/// HS256 User JWT minted for bridge/gateway consumption (inner `nats.jwt` on wire responses).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MintedUserJwt(String);

impl MintedUserJwt {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub struct SigningKey(pub(crate) EncodingKey);

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SigningKey")
    }
}

impl SigningKey {
    pub fn from_secret(secret: &[u8]) -> Self {
        Self(EncodingKey::from_secret(secret))
    }
}

#[derive(Debug)]
pub enum JwtError {
    Encode(jsonwebtoken::errors::Error),
    Decode(jsonwebtoken::errors::Error),
    SystemTime(std::time::SystemTimeError),
    InvalidCallerId,
    InvalidExternalSubject,
    IssuedAtOutOfRange,
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
        }
    }
}

impl std::error::Error for JwtError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Encode(e) | Self::Decode(e) => Some(e),
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
    pub sub: ExternalSubject,
    pub aud: AccountName,
    pub data: SpiceDbPrincipal,
    pub caller_id: CallerId,
    #[serde(default = "default_permissions_for_serde_back_compat")]
    pub nats_permissions: IssuedPermissions,
}

fn default_permissions_for_serde_back_compat() -> IssuedPermissions {
    IssuedPermissions {
        publish_allow: Vec::new(),
        subscribe_allow: Vec::new(),
    }
}

impl UserJwtClaims {
    pub fn mint(
        &self,
        signing_key: &SigningKey,
        issued_at: SystemTime,
        ttl: Duration,
    ) -> Result<MintedUserJwt, JwtError> {
        let iat_secs = secs_since_unix(issued_at)?;
        let ttl_secs_i64 = i64::try_from(ttl.as_secs().max(1)).unwrap_or(i64::MAX);
        let exp_secs = iat_secs.saturating_add(ttl_secs_i64);

        #[derive(Serialize)]
        struct Claims<'a> {
            sub: &'a str,
            aud: &'a str,
            caller_id: &'a str,
            data: &'a Value,
            nats_permissions: &'a IssuedPermissions,
            exp: i64,
            iat: i64,
            nbf: i64,
        }

        let claims = Claims {
            sub: self.sub.as_str(),
            aud: self.aud.as_str(),
            caller_id: self.caller_id.as_str(),
            data: &self.data.0,
            nats_permissions: &self.nats_permissions,
            exp: exp_secs,
            iat: iat_secs,
            nbf: iat_secs,
        };

        encode(&Header::new(Algorithm::HS256), &claims, &signing_key.0)
            .map(MintedUserJwt::new)
            .map_err(JwtError::Encode)
    }

    #[cfg(test)]
    fn mint_for_test_ttl(
        &self,
        signing_key: &SigningKey,
        ttl: Duration,
    ) -> Result<MintedUserJwt, JwtError> {
        self.mint(signing_key, UNIX_EPOCH + Duration::from_secs(1_000), ttl)
    }
}

/// Reads `caller_id` from a freshly minted User JWT without signature verification.
///
/// The bridge uses this only on tokens it just received from the auth callout mint path.
pub fn caller_id_from_minted_jwt(token: &str) -> Result<CallerId, JwtError> {
    #[derive(Deserialize)]
    struct Payload {
        caller_id: String,
    }

    let mut validation = Validation::new(Algorithm::HS256);
    validation.insecure_disable_signature_validation();
    validation.validate_exp = false;
    validation.validate_aud = false;
    let decoded = decode::<Payload>(token, &DecodingKey::from_secret(b""), &validation)
        .map_err(JwtError::Decode)?;
    CallerId::new(decoded.claims.caller_id)
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

fn secs_since_unix(t: SystemTime) -> Result<i64, JwtError> {
    let secs = t.duration_since(UNIX_EPOCH).map_err(JwtError::SystemTime)?.as_secs();
    i64::try_from(secs).map_err(|_| JwtError::IssuedAtOutOfRange)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};

    #[derive(Debug, serde::Deserialize)]
    struct ParsedMinted {
        sub: String,
        aud: String,
        caller_id: String,
        data: serde_json::Value,
    }

    #[test]
    fn mint_decodes_expected_claims() {
        let signing_key = SigningKey::from_secret(b"secret-for-hs256-test");
        let caller_id = CallerId::new("caller1").unwrap();
        let claims = UserJwtClaims {
            sub: ExternalSubject::new("alice").unwrap(),
            aud: AccountName::new("tenant-acme"),
            data: SpiceDbPrincipal(json!({"spicedb_subject": "user/alice"})),
            nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
            caller_id,
        };
        let token = claims
            .mint_for_test_ttl(&signing_key, Duration::from_secs(60))
            .unwrap();
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = false;
        validation.validate_aud = false;
        let decoded = decode::<ParsedMinted>(
            token.as_str(),
            &DecodingKey::from_secret(b"secret-for-hs256-test"),
            &validation,
        )
        .unwrap();
        assert_eq!(decoded.claims.sub, "alice");
        assert_eq!(decoded.claims.aud, "tenant-acme");
        assert_eq!(decoded.claims.caller_id, "caller1");
        assert_eq!(
            decoded.claims.data.get("spicedb_subject"),
            Some(&json!("user/alice"))
        );
    }

    #[test]
    fn mint_wrong_alg_fails_decode() {
        let signing_key = SigningKey::from_secret(b"a");
        let caller_id = CallerId::new("cid").unwrap();
        let claims = UserJwtClaims {
            sub: ExternalSubject::new("alice").unwrap(),
            aud: AccountName::new("tenant-acme"),
            data: SpiceDbPrincipal(json!({})),
            nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
            caller_id,
        };
        let token = claims
            .mint_for_test_ttl(&signing_key, Duration::from_secs(10))
            .unwrap();
        let mut validation = Validation::new(Algorithm::RS384);
        validation.validate_exp = false;
        validation.validate_aud = false;
        let err =
            decode::<ParsedMinted>(token.as_str(), &DecodingKey::from_secret(b"a"), &validation).unwrap_err();
        assert!(
            matches!(err.kind(), jsonwebtoken::errors::ErrorKind::InvalidAlgorithm)
                || err.to_string().to_lowercase().contains("algorithm"),
            "{err:?}"
        );
    }

    #[test]
    fn mint_rejects_wrong_verification_key() {
        let signing_key = SigningKey::from_secret(b"signer-a------------------------");
        let caller_id = CallerId::new("cid").unwrap();
        let claims = UserJwtClaims {
            sub: ExternalSubject::new("s").unwrap(),
            aud: AudienceAccount::new("a"),
            data: SpiceDbPrincipal(json!({})),
            nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
            caller_id,
        };
        let token = claims
            .mint_for_test_ttl(&signing_key, Duration::from_secs(60))
            .unwrap();
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = false;
        validation.validate_aud = false;
        let wrong = decode::<ParsedMinted>(
            token.as_str(),
            &DecodingKey::from_secret(b"signer-b------------------------"),
            &validation,
        );
        assert!(wrong.is_err());
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
