use trogon_credential_domain::AuditCorrelationId;
use trogon_std::SecretString;

use super::constants::{CORRELATION_ID_HEADER, TOKEN_HEADER};

/// The identity one OpenBao call is made as: the token it sends and, for a
/// release, the correlation id OpenBao records alongside it.
pub(crate) struct OpenBaoCaller {
    token: SecretString,
    correlation_id: Option<AuditCorrelationId>,
}

impl OpenBaoCaller {
    pub(crate) fn new(token: SecretString) -> Self {
        Self {
            token,
            correlation_id: None,
        }
    }

    pub(crate) fn correlated(token: SecretString, correlation_id: AuditCorrelationId) -> Self {
        Self {
            token,
            correlation_id: Some(correlation_id),
        }
    }

    pub(crate) fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let request = request.header(TOKEN_HEADER, self.token.as_str());
        match &self.correlation_id {
            Some(correlation_id) => request.header(CORRELATION_ID_HEADER, correlation_id.as_str()),
            None => request,
        }
    }
}
