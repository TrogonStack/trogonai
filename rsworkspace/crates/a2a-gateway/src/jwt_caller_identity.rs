use a2a_auth_callout::{SpiceDbPrincipal, SpiceDbSubject};
use async_nats::HeaderMap;
use tracing::warn;

use a2a_nats::constants::{GATEWAY_CALLER_ID_HEADER, GATEWAY_PRINCIPAL_HEADER};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerIdentitySource {
    HeaderTrusted,
    JwtDataClaim,
}

impl CallerIdentitySource {
    pub fn audit_label(self) -> &'static str {
        match self {
            Self::HeaderTrusted => "header_trusted",
            Self::JwtDataClaim => "jwt_data_claim",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwtCallerIdentity {
    pub spicedb_subject: SpiceDbSubject,
    pub audience: Option<String>,
    pub source: CallerIdentitySource,
}

pub fn resolve_gateway_caller_identity(headers: &HeaderMap) -> Option<JwtCallerIdentity> {
    if let Some(principal) = principal_from_headers(headers)
        && let Some(subject) = principal.spicedb_subject()
    {
        return Some(JwtCallerIdentity {
            spicedb_subject: subject,
            audience: audience_from_principal(&principal),
            source: CallerIdentitySource::JwtDataClaim,
        });
    }

    let raw = header_value(headers, GATEWAY_CALLER_ID_HEADER)?;

    warn!(
        caller_id_header = raw.as_str(),
        "gateway caller identity: using deprecated X-A2a-Caller-Id header trust; migrate to X-A2a-Spicedb-Principal from auth-callout mint"
    );

    Some(JwtCallerIdentity {
        spicedb_subject: SpiceDbSubject::new(raw),
        audience: None,
        source: CallerIdentitySource::HeaderTrusted,
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

fn audience_from_principal(principal: &SpiceDbPrincipal) -> Option<String> {
    principal
        .0
        .get("aud")
        .or_else(|| principal.0.get("audience"))
        .and_then(|v| v.as_str())
        .map(str::to_owned)
        .filter(|s| !s.is_empty())
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

    fn principal_headers(subject: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            GATEWAY_PRINCIPAL_HEADER,
            format!(r#"{{"spicedb_subject":"{subject}","aud":"tenant-acme"}}"#),
        );
        headers
    }

    #[test]
    fn jwt_principal_header_wins_with_jwt_data_claim_source() {
        let mut headers = principal_headers("spicedb-user-1");
        headers.insert(GATEWAY_CALLER_ID_HEADER, "legacy-caller");

        let identity = resolve_gateway_caller_identity(&headers).expect("identity");
        assert_eq!(identity.spicedb_subject.as_str(), "spicedb-user-1");
        assert_eq!(identity.source, CallerIdentitySource::JwtDataClaim);
        assert_eq!(identity.audience.as_deref(), Some("tenant-acme"));

        let (caller_id, source) = gateway_audit_caller_attribution(Some(identity));
        assert_eq!(caller_id, "spicedb-user-1");
        assert_eq!(source.as_deref(), Some("jwt_data_claim"));
    }

    #[test]
    fn caller_id_header_only_uses_header_trusted_source() {
        let mut headers = HeaderMap::new();
        headers.insert(GATEWAY_CALLER_ID_HEADER, "bridge-caller");

        let identity = resolve_gateway_caller_identity(&headers).expect("identity");
        assert_eq!(identity.spicedb_subject.as_str(), "bridge-caller");
        assert_eq!(identity.source, CallerIdentitySource::HeaderTrusted);

        let (caller_id, source) = gateway_audit_caller_attribution(Some(identity));
        assert_eq!(caller_id, "bridge-caller");
        assert_eq!(source.as_deref(), Some("header_trusted"));
    }

    #[test]
    fn neither_header_yields_audit_fallback() {
        let headers = HeaderMap::new();
        assert!(resolve_gateway_caller_identity(&headers).is_none());

        let (caller_id, source) = gateway_audit_caller_attribution(None);
        assert_eq!(caller_id, "_");
        assert!(source.is_none());
    }
}
