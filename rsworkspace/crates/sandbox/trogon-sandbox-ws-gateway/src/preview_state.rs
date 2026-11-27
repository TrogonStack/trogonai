use std::time::Duration;

use trogon_sandbox_contract::{GetSandboxDomainResponse, LeaseId};

use crate::constants::{MIN_PREVIEW_TTL, PREVIEW_CONNECT_TIMEOUT, PREVIEW_GRANT_PREFIX, PREVIEW_ROUTE_PREFIX};
use crate::encode_segment::encode_segment;
use crate::session_record::now_secs;
use crate::session_token::session_token_hash;
use crate::{PreviewClaims, PreviewConfigError, PreviewGrantError, PreviewKeys, SessionRecord, SessionStore};

/// Everything the gateway needs to mint and serve previews.
pub struct PreviewState {
    keys: PreviewKeys,
    public_url: String,
    ttl: Duration,
    client: reqwest::Client,
}

impl PreviewState {
    /// `public_url` is the externally reachable base of this gateway, for example `https://sandbox.example.com`.
    /// `ttl` is clamped to `max_ttl`, the session store's retention, which also bounds how long a stored grant
    /// survives.
    pub fn new(
        keys: PreviewKeys,
        public_url: &str,
        ttl: Duration,
        max_ttl: Duration,
    ) -> Result<Self, PreviewConfigError> {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(PREVIEW_CONNECT_TIMEOUT)
            .build()?;
        Ok(Self {
            keys,
            public_url: public_url.trim_end_matches('/').to_string(),
            ttl: ttl.min(max_ttl).max(MIN_PREVIEW_TTL),
            client,
        })
    }

    pub fn keys(&self) -> &PreviewKeys {
        &self.keys
    }

    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// The key a preview token's grant is stored under.
    pub(crate) fn grant_key(token: &str) -> Result<String, PreviewGrantError> {
        Ok(session_token_hash(&format!("{PREVIEW_GRANT_PREFIX}{token}"))?)
    }

    /// Fills `url` and `expires_at` on an `Available` response, storing the grant the proxy later checks. Other
    /// outcomes pass through untouched.
    pub async fn decorate(
        &self,
        session: &SessionRecord,
        sessions: &dyn SessionStore,
        lease_id: &LeaseId,
        port: u16,
        response: GetSandboxDomainResponse,
    ) -> Result<GetSandboxDomainResponse, PreviewGrantError> {
        let GetSandboxDomainResponse::Available { endpoint, access, .. } = response else {
            return Ok(response);
        };
        let expires_at = now_secs() + self.ttl.as_secs();
        let token = self.keys.issue(&PreviewClaims {
            tenant: session.tenant.clone(),
            lease_id: lease_id.clone(),
            port,
            expires_at,
        })?;
        let grant = SessionRecord {
            tenant: session.tenant.clone(),
            service_account: session.service_account.clone(),
            api_key: session.api_key.clone(),
            expires_at,
        };
        sessions.put(&Self::grant_key(&token)?, grant).await?;
        let url = format!(
            "{}{PREVIEW_ROUTE_PREFIX}/{}/{port}/{token}/",
            self.public_url,
            encode_segment(lease_id.as_str()),
        );
        Ok(GetSandboxDomainResponse::Available {
            endpoint,
            url: Some(url),
            expires_at: Some(expires_at),
            access,
        })
    }
}
