use std::sync::Once;

use a2a_auth_callout::{SpiceDbPrincipal, SpiceDbSubject};
use async_nats::HeaderMap;
use tracing::warn;
use trogon_std::env::ReadEnv;

use a2a_nats::constants::{GATEWAY_CALLER_ID_HEADER, GATEWAY_PRINCIPAL_HEADER};

pub const ENV_GATEWAY_TRUST_CALLER_HEADERS: &str = "A2A_GATEWAY_TRUST_CALLER_HEADERS";

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

pub trait ConnectionCallerIdentitySource {
    fn verified_caller_identity(&self) -> Option<VerifiedCallerIdentity>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct UnavailableConnectionCallerIdentity;

impl ConnectionCallerIdentitySource for UnavailableConnectionCallerIdentity {
    fn verified_caller_identity(&self) -> Option<VerifiedCallerIdentity> {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerIdentitySource {
    ConnectionJwtDataClaim,
    HeaderPrincipal,
    HeaderCallerId,
}

impl CallerIdentitySource {
    pub fn audit_label(self) -> &'static str {
        match self {
            Self::ConnectionJwtDataClaim => "jwt_data_claim",
            Self::HeaderPrincipal => "header_principal",
            Self::HeaderCallerId => "header_trusted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtCallerIdentity {
    pub spicedb_subject: SpiceDbSubject,
    pub audience: Option<String>,
    pub source: CallerIdentitySource,
}

pub fn resolve_gateway_caller_identity(
    connection: &impl ConnectionCallerIdentitySource,
    headers: &HeaderMap,
    policy: GatewayCallerIdentityPolicy,
) -> Option<JwtCallerIdentity> {
    if let Some(verified) = connection.verified_caller_identity() {
        return identity_from_verified(verified, CallerIdentitySource::ConnectionJwtDataClaim);
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
        spicedb_subject: SpiceDbSubject::new(raw),
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
    use a2a_nats::constants::{GATEWAY_CALLER_ID_HEADER, GATEWAY_PRINCIPAL_HEADER};
    use async_nats::HeaderMap;

    struct TestConnection {
        verified: Option<VerifiedCallerIdentity>,
    }

    impl ConnectionCallerIdentitySource for TestConnection {
        fn verified_caller_identity(&self) -> Option<VerifiedCallerIdentity> {
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

    #[test]
    fn connection_verified_principal_wins_over_headers() {
        let principal = SpiceDbPrincipal::new("conn-user");
        let connection = TestConnection {
            verified: VerifiedCallerIdentity::from_principal(principal),
        };
        let headers = principal_headers("header-user");

        let identity = resolve_gateway_caller_identity(&connection, &headers, trust_headers_on())
            .expect("identity");
        assert_eq!(identity.spicedb_subject.as_str(), "conn-user");
        assert_eq!(identity.source, CallerIdentitySource::ConnectionJwtDataClaim);
    }

    #[test]
    fn trust_headers_off_ignores_principal_and_caller_id_headers() {
        let connection = TestConnection { verified: None };
        let mut headers = principal_headers("spicedb-user-1");
        headers.insert(GATEWAY_CALLER_ID_HEADER, "legacy-caller");

        assert!(resolve_gateway_caller_identity(&connection, &headers, trust_headers_off()).is_none());
    }

    #[test]
    fn trust_headers_on_principal_header_uses_header_principal_source() {
        let connection = TestConnection { verified: None };
        let mut headers = principal_headers("spicedb-user-1");
        headers.insert(GATEWAY_CALLER_ID_HEADER, "legacy-caller");

        let identity = resolve_gateway_caller_identity(&connection, &headers, trust_headers_on())
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
        let connection = TestConnection { verified: None };
        let mut headers = HeaderMap::new();
        headers.insert(GATEWAY_CALLER_ID_HEADER, "bridge-caller");

        let identity = resolve_gateway_caller_identity(&connection, &headers, trust_headers_on())
            .expect("identity");
        assert_eq!(identity.spicedb_subject.as_str(), "bridge-caller");
        assert_eq!(identity.source, CallerIdentitySource::HeaderCallerId);

        let (caller_id, source) = gateway_audit_caller_attribution(Some(identity));
        assert_eq!(caller_id, "bridge-caller");
        assert_eq!(source.as_deref(), Some("header_trusted"));
    }

    #[test]
    fn neither_connection_nor_trusted_headers_yields_audit_fallback() {
        let connection = TestConnection { verified: None };
        let headers = HeaderMap::new();
        assert!(resolve_gateway_caller_identity(&connection, &headers, trust_headers_off()).is_none());

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
}
