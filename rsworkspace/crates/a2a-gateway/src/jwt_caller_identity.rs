use std::sync::Arc;
use std::sync::Once;

use a2a_auth_callout::{
    caller_jwt_header::CallerJwtHeaderValue, AccountName, AudienceAccount, SigningKeySource, SpiceDbPrincipal,
    UserJwtClaims, CALLER_JWT_HEADER_NAME,
};
use async_nats::HeaderMap;
use tracing::warn;
use trogon_std::env::ReadEnv;

use a2a_nats::constants::{GATEWAY_CALLER_ID_HEADER, GATEWAY_PRINCIPAL_HEADER};

pub const ENV_GATEWAY_TRUST_CALLER_HEADERS: &str = "A2A_GATEWAY_TRUST_CALLER_HEADERS";
pub const ENV_GATEWAY_JWT_AUDIENCE: &str = "A2A_GATEWAY_JWT_AUDIENCE";

static TRUST_CALLER_HEADERS_WARN: Once = Once::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustCallerHeaders(bool);

impl TrustCallerHeaders {
    pub fn is_enabled(self) -> bool {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GatewayCallerIdentityPolicy {
    pub trust_caller_headers: TrustCallerHeaders,
}

impl GatewayCallerIdentityPolicy {
    pub fn production_default() -> Self {
        Self {
            trust_caller_headers: TrustCallerHeaders(false),
        }
    }
}

pub fn gateway_caller_identity_policy<E: ReadEnv>(env: &E) -> GatewayCallerIdentityPolicy {
    let trust_caller_headers = TrustCallerHeaders(gateway_trust_caller_headers_enabled(env));
    if trust_caller_headers.is_enabled() {
        TRUST_CALLER_HEADERS_WARN.call_once(|| {
            warn!(
                env = ENV_GATEWAY_TRUST_CALLER_HEADERS,
                "gateway caller identity: labs-only header-trust fallback active; disable for production"
            );
        });
    }
    GatewayCallerIdentityPolicy {
        trust_caller_headers,
    }
}

fn gateway_trust_caller_headers_enabled<E: ReadEnv>(env: &E) -> bool {
    env.var(ENV_GATEWAY_TRUST_CALLER_HEADERS)
        .ok()
        .is_some_and(|flag| matches!(flag.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

pub fn gateway_jwt_audience<E: ReadEnv>(env: &E, prefix_fallback: &str) -> AudienceAccount {
    env.var(ENV_GATEWAY_JWT_AUDIENCE)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(|s| AccountName::new(s.trim()))
        .unwrap_or_else(|| AccountName::new(prefix_fallback))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedCallerIdentity {
    principal: SpiceDbPrincipal,
    audience: Option<AudienceFromPrincipal>,
}

impl VerifiedCallerIdentity {
    pub fn new(principal: SpiceDbPrincipal, audience: Option<AudienceFromPrincipal>) -> Self {
        Self { principal, audience }
    }

    pub fn principal(&self) -> &SpiceDbPrincipal {
        &self.principal
    }

    pub fn audience(&self) -> Option<&AudienceFromPrincipal> {
        self.audience.as_ref()
    }

    pub fn from_principal(principal: SpiceDbPrincipal) -> Option<Self> {
        principal.spicedb_subject()?;
        let audience = audience_from_principal(&principal);
        Some(Self { principal, audience })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudienceFromPrincipal(String);

impl AudienceFromPrincipal {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub trait MessageCallerIdentitySource {
    fn verified_caller_identity(&self, message: &async_nats::Message) -> Option<VerifiedCallerIdentity>;
}

pub struct JwtHeaderCallerIdentitySource {
    signing_key_source: Arc<dyn SigningKeySource>,
    audience: AudienceAccount,
}

impl JwtHeaderCallerIdentitySource {
    pub fn new(signing_key_source: Arc<dyn SigningKeySource>, audience: AudienceAccount) -> Self {
        Self {
            signing_key_source,
            audience,
        }
    }
}

impl MessageCallerIdentitySource for JwtHeaderCallerIdentitySource {
    fn verified_caller_identity(&self, message: &async_nats::Message) -> Option<VerifiedCallerIdentity> {
        let headers = message.headers.as_ref()?;
        let raw = header_value(headers, CALLER_JWT_HEADER_NAME)?;
        let token = CallerJwtHeaderValue::parse(raw).ok()?;
        let claims =
            UserJwtClaims::verify_minted_user_jwt(token.as_str(), self.signing_key_source.as_ref(), &self.audience)
                .ok()?;
        VerifiedCallerIdentity::from_principal(claims.data)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerIdentitySource {
    MessageCallerJwtHeader,
    HeaderPrincipal,
    HeaderCallerId,
}

impl CallerIdentitySource {
    pub fn audit_label(self) -> &'static str {
        match self {
            Self::MessageCallerJwtHeader => "jwt_header",
            Self::HeaderPrincipal => "header_principal",
            Self::HeaderCallerId => "header_trusted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtCallerIdentity {
    pub spicedb_subject: a2a_auth_callout::SpiceDbSubject,
    pub audience: Option<String>,
    pub source: CallerIdentitySource,
}

pub fn resolve_gateway_caller_identity(
    message_identity: &impl MessageCallerIdentitySource,
    message: &async_nats::Message,
    headers: &HeaderMap,
    policy: GatewayCallerIdentityPolicy,
) -> Option<JwtCallerIdentity> {
    if let Some(verified) = message_identity.verified_caller_identity(message) {
        return identity_from_verified(verified, CallerIdentitySource::MessageCallerJwtHeader);
    }

    if !policy.trust_caller_headers.is_enabled() {
        return None;
    }

    if let Some(principal) = principal_from_headers(headers)
        && let Some(subject) = principal.spicedb_subject()
    {
        return Some(JwtCallerIdentity {
            spicedb_subject: subject,
            audience: audience_from_principal(&principal).map(|a| a.0),
            source: CallerIdentitySource::HeaderPrincipal,
        });
    }

    let raw = header_value(headers, GATEWAY_CALLER_ID_HEADER)?;

    Some(JwtCallerIdentity {
        spicedb_subject: a2a_auth_callout::SpiceDbSubject::new(raw),
        audience: None,
        source: CallerIdentitySource::HeaderCallerId,
    })
}

fn identity_from_verified(
    verified: VerifiedCallerIdentity,
    source: CallerIdentitySource,
) -> Option<JwtCallerIdentity> {
    let subject = verified.principal.spicedb_subject()?;
    Some(JwtCallerIdentity {
        spicedb_subject: subject,
        audience: verified.audience.map(|a| a.0),
        source,
    })
}

pub fn gateway_audit_caller_attribution(identity: Option<JwtCallerIdentity>) -> (String, Option<String>) {
    match identity {
        Some(id) => (
            id.spicedb_subject.as_str().to_owned(),
            Some(id.source.audit_label().to_owned()),
        ),
        None => ("_".to_owned(), None),
    }
}

fn principal_from_headers(headers: &HeaderMap) -> Option<SpiceDbPrincipal> {
    let raw = header_value(headers, GATEWAY_PRINCIPAL_HEADER)?;
    let value = serde_json::from_str(&raw).ok()?;
    Some(SpiceDbPrincipal(value))
}

fn audience_from_principal(principal: &SpiceDbPrincipal) -> Option<AudienceFromPrincipal> {
    principal
        .0
        .get("aud")
        .or_else(|| principal.0.get("audience"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| AudienceFromPrincipal(s.to_owned()))
}

fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| std::str::from_utf8(value.as_ref()).ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use a2a_auth_callout::{
        jwt::ExternalSubject, CallerId, IssuedPermissions, KeyVersion, StaticSigningKeySource, UserJwtSubject,
    };
    use a2a_nats::constants::{GATEWAY_CALLER_ID_HEADER, GATEWAY_PRINCIPAL_HEADER};
    use async_nats::HeaderMap;
    use nkeys::KeyPair;
    use serde_json::json;
    use std::time::{Duration, SystemTime};

    struct TestMessageIdentity {
        verified: Option<VerifiedCallerIdentity>,
    }

    impl MessageCallerIdentitySource for TestMessageIdentity {
        fn verified_caller_identity(&self, _message: &async_nats::Message) -> Option<VerifiedCallerIdentity> {
            self.verified.clone()
        }
    }

    fn principal_headers(subject: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            GATEWAY_PRINCIPAL_HEADER,
            format!(r#"{{"spicedb_subject":"{subject}","aud":"tenant-acme"}}"#),
        );
        headers
    }

    fn trust_headers_on() -> GatewayCallerIdentityPolicy {
        GatewayCallerIdentityPolicy {
            trust_caller_headers: TrustCallerHeaders(true),
        }
    }

    fn trust_headers_off() -> GatewayCallerIdentityPolicy {
        GatewayCallerIdentityPolicy::production_default()
    }

    fn empty_message() -> async_nats::Message {
        async_nats::Message {
            subject: "a2a.gateway.bot.message.send".into(),
            reply: Some("_INBOX.reply".into()),
            payload: vec![].into(),
            headers: None,
            status: None,
            description: None,
            length: 0,
        }
    }

    #[test]
    fn message_verified_principal_wins_over_headers() {
        let principal = SpiceDbPrincipal::new("msg-user");
        let message_identity = TestMessageIdentity {
            verified: VerifiedCallerIdentity::from_principal(principal),
        };
        let headers = principal_headers("header-user");
        let msg = empty_message();

        let identity = resolve_gateway_caller_identity(&message_identity, &msg, &headers, trust_headers_on())
            .expect("identity");
        assert_eq!(identity.spicedb_subject.as_str(), "msg-user");
        assert_eq!(identity.source, CallerIdentitySource::MessageCallerJwtHeader);
    }

    #[test]
    fn trust_headers_off_ignores_principal_and_caller_id_headers() {
        let message_identity = TestMessageIdentity { verified: None };
        let mut headers = principal_headers("spicedb-user-1");
        headers.insert(GATEWAY_CALLER_ID_HEADER, "legacy-caller");
        let msg = empty_message();

        assert!(resolve_gateway_caller_identity(
            &message_identity,
            &msg,
            &headers,
            trust_headers_off()
        )
        .is_none());
    }

    #[test]
    fn trust_headers_on_principal_header_uses_header_principal_source() {
        let message_identity = TestMessageIdentity { verified: None };
        let mut headers = principal_headers("spicedb-user-1");
        headers.insert(GATEWAY_CALLER_ID_HEADER, "legacy-caller");
        let msg = empty_message();

        let identity = resolve_gateway_caller_identity(&message_identity, &msg, &headers, trust_headers_on())
            .expect("identity");
        assert_eq!(identity.spicedb_subject.as_str(), "spicedb-user-1");
        assert_eq!(identity.source, CallerIdentitySource::HeaderPrincipal);
        assert_eq!(identity.audience.as_deref(), Some("tenant-acme"));

        let (caller_id, source) = gateway_audit_caller_attribution(Some(identity));
        assert_eq!(caller_id, "spicedb-user-1");
        assert_eq!(source.as_deref(), Some("header_principal"));
    }

    #[test]
    fn trust_headers_on_caller_id_header_only_uses_header_caller_id_source() {
        let message_identity = TestMessageIdentity { verified: None };
        let mut headers = HeaderMap::new();
        headers.insert(GATEWAY_CALLER_ID_HEADER, "bridge-caller");
        let msg = empty_message();

        let identity = resolve_gateway_caller_identity(&message_identity, &msg, &headers, trust_headers_on())
            .expect("identity");
        assert_eq!(identity.spicedb_subject.as_str(), "bridge-caller");
        assert_eq!(identity.source, CallerIdentitySource::HeaderCallerId);

        let (caller_id, source) = gateway_audit_caller_attribution(Some(identity));
        assert_eq!(caller_id, "bridge-caller");
        assert_eq!(source.as_deref(), Some("header_trusted"));
    }

    #[test]
    fn neither_message_nor_trusted_headers_yields_audit_fallback() {
        let message_identity = TestMessageIdentity { verified: None };
        let headers = HeaderMap::new();
        let msg = empty_message();
        assert!(resolve_gateway_caller_identity(
            &message_identity,
            &msg,
            &headers,
            trust_headers_off()
        )
        .is_none());

        let (caller_id, source) = gateway_audit_caller_attribution(None);
        assert_eq!(caller_id, "_");
        assert!(source.is_none());
    }

    #[test]
    fn trust_caller_headers_env_defaults_off() {
        struct EmptyEnv;
        impl ReadEnv for EmptyEnv {
            fn var(&self, _key: &str) -> Result<String, std::env::VarError> {
                Err(std::env::VarError::NotPresent)
            }
        }
        assert!(!gateway_caller_identity_policy(&EmptyEnv)
            .trust_caller_headers
            .is_enabled());
    }

    #[test]
    fn jwt_header_source_verifies_minted_token() {
        let issuer = KeyPair::new_account();
        let user = KeyPair::new_user();
        let issuer_seed = issuer.seed().expect("issuer seed");
        let source = Arc::new(
            StaticSigningKeySource::new(
                issuer_seed.as_str(),
                KeyVersion::new("current").expect("version"),
            )
            .expect("static source"),
        );
        let caller_id = CallerId::new("caller1").expect("caller");
        let claims = UserJwtClaims {
            kid: KeyVersion::new("current").expect("version"),
            sub: ExternalSubject::new("alice").expect("sub"),
            aud: AccountName::new("tenant-acme"),
            data: SpiceDbPrincipal(json!({"spicedb_subject": "jwt-header-user"})),
            nats_permissions: IssuedPermissions::default_for_caller(&caller_id),
            caller_id,
        };
        let subject =
            UserJwtSubject::from_user_nkey(a2a_auth_callout::NkeyPublic::parse(user.public_key()).unwrap());
        let handle = source.current();
        let issued_at = SystemTime::now();
        let token = claims
            .mint(
                &handle.minting_material(),
                &subject,
                issued_at,
                Duration::from_secs(300),
            )
            .expect("mint");

        let mut headers = HeaderMap::new();
        headers.insert(
            CALLER_JWT_HEADER_NAME,
            CallerJwtHeaderValue::from_minted(&token).as_str(),
        );
        let mut msg = empty_message();
        msg.headers = Some(headers);

        let identity_source =
            JwtHeaderCallerIdentitySource::new(source, AccountName::new("tenant-acme"));
        let verified = identity_source.verified_caller_identity(&msg).expect("verified");
        assert_eq!(
            verified.principal().spicedb_subject().unwrap().as_str(),
            "jwt-header-user"
        );
    }
}
