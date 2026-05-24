use a2a_auth_callout::SpiceDbPrincipal;
use async_nats::HeaderMap;

use crate::constants::GATEWAY_PRINCIPAL_HEADER;
use crate::push::{CallerId, resolve_push_dlq_caller_id};

/// Optional minted JWT `data` claim threaded from gateway ingress into agent dispatch.
///
/// Populated from NATS header [`GATEWAY_PRINCIPAL_HEADER`] when auth-callout + gateway forward
/// the principal; absent today on direct agent connects without that path.
#[derive(Clone, Debug)]
pub struct PrincipalCarrier {
    principal: Option<SpiceDbPrincipal>,
    fallback_caller_id: CallerId,
}

impl PrincipalCarrier {
    pub fn absent(fallback_caller_id: CallerId) -> Self {
        Self {
            principal: None,
            fallback_caller_id,
        }
    }

    pub fn with_principal(principal: SpiceDbPrincipal, fallback_caller_id: CallerId) -> Self {
        Self {
            principal: Some(principal),
            fallback_caller_id,
        }
    }

    pub fn from_nats_headers(headers: Option<&HeaderMap>, fallback_caller_id: CallerId) -> Self {
        Self {
            principal: headers.and_then(principal_from_headers),
            fallback_caller_id,
        }
    }

    pub fn principal(&self) -> Option<&SpiceDbPrincipal> {
        self.principal.as_ref()
    }

    pub fn push_dlq_caller_id(&self) -> CallerId {
        resolve_push_dlq_caller_id(self.principal.as_ref(), &self.fallback_caller_id)
    }
}

fn principal_from_headers(headers: &HeaderMap) -> Option<SpiceDbPrincipal> {
    let raw = headers.get(GATEWAY_PRINCIPAL_HEADER)?.as_str();
    let value = serde_json::from_str(raw).ok()?;
    Some(SpiceDbPrincipal(value))
}

#[cfg(test)]
pub(crate) fn principal_header_fixture(subject: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        GATEWAY_PRINCIPAL_HEADER,
        format!(r#"{{"spicedb_subject":"{subject}"}}"#),
    );
    headers
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::DEFAULT_PUSH_DLQ_CALLER_SEGMENT;

    fn fallback() -> CallerId {
        CallerId::default()
    }

    #[test]
    fn absent_carrier_uses_fallback_caller_id() {
        let carrier = PrincipalCarrier::absent(fallback());
        assert_eq!(carrier.push_dlq_caller_id().as_str(), DEFAULT_PUSH_DLQ_CALLER_SEGMENT);
    }

    #[test]
    fn header_fixture_parses_spicedb_subject() {
        let headers = principal_header_fixture("p.q");
        let carrier = PrincipalCarrier::from_nats_headers(Some(&headers), fallback());
        assert_eq!(carrier.push_dlq_caller_id().as_str(), "p_q");
    }

    #[test]
    fn malformed_header_is_treated_as_absent_principal() {
        let mut headers = HeaderMap::new();
        headers.insert(GATEWAY_PRINCIPAL_HEADER, "not-json");
        let carrier = PrincipalCarrier::from_nats_headers(Some(&headers), fallback());
        assert_eq!(carrier.push_dlq_caller_id().as_str(), DEFAULT_PUSH_DLQ_CALLER_SEGMENT);
    }
}
