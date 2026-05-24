use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use a2a_auth_callout::{
    AuthDispatcher, BridgeAuthScheme, BridgeConnectOpts, BridgeMintRequest, BridgeMintResponse,
    ServerAuthRequestClaims,
};

use super::{AuthMintWire, BytesPayload};
use crate::error::BridgeError;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BridgeTenantAccount(String);

impl BridgeTenantAccount {
    pub fn new(account: impl Into<String>) -> Result<Self, BridgeError> {
        let raw = account.into();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(BridgeError::Mint("bridge tenant account must be non-empty".into()));
        }
        Ok(Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// In-process [`AuthMintWire`] that delegates to [`CalloutDispatcher`] (no live NATS).
pub struct InProcessCalloutDispatcherMintWire {
    dispatcher: Arc<dyn AuthDispatcher>,
    tenant: BridgeTenantAccount,
    mint_count: Arc<AtomicUsize>,
}

impl InProcessCalloutDispatcherMintWire {
    pub fn new(dispatcher: Arc<dyn AuthDispatcher>, tenant: BridgeTenantAccount) -> Self {
        Self {
            dispatcher,
            tenant,
            mint_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[must_use]
    pub fn mint_count(&self) -> usize {
        self.mint_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl AuthMintWire for InProcessCalloutDispatcherMintWire {
    async fn roundtrip_message(&self, _subject: String, payload: BytesPayload) -> Result<Vec<u8>, BridgeError> {
        self.mint_count.fetch_add(1, Ordering::SeqCst);
        let mut request: BridgeMintRequest =
            serde_json::from_slice(&payload.0).map_err(|e: serde_json::Error| BridgeError::Deserialize(e))?;
        if request.account.is_none() {
            request.account = Some(self.tenant.as_str().to_owned());
        }
        if request.connect_opts.is_none() && request.user_jwt.is_some() {
            request.connect_opts = Some(BridgeConnectOpts {
                auth_scheme: Some(BridgeAuthScheme::Oidc),
                api_key: None,
            });
        }
        let claims = ServerAuthRequestClaims::from_bridge_mint(request)
            .map_err(|e| BridgeError::Mint(e.to_string()))?;
        let user_jwt = self
            .dispatcher
            .dispatch(claims)
            .await
            .map_err(|e| BridgeError::Mint(e.to_string()))?;
        let response = BridgeMintResponse {
            user_jwt: user_jwt.as_str().to_owned(),
        };
        serde_json::to_vec(&response).map_err(|e: serde_json::Error| BridgeError::Serialize(e))
    }
}

#[cfg(test)]
pub(crate) fn harness_callout_dispatcher(caller_id: &str) -> a2a_auth_callout::CalloutDispatcher {
    use std::sync::Arc;
    use std::time::Duration;

    use a2a_auth_callout::{
        CalloutDispatcher, CalloutDispatcherConfig, StaticAccountResolver,
    };
    use a2a_auth_callout::credentials::oidc::{BearerToken, OidcVerifier};
    use a2a_auth_callout::jwt::{
        AudienceAccount, CallerId, ExternalSubject, SigningKey, SpiceDbPrincipal, UserJwtClaims,
    };
    use a2a_auth_callout::permissions::IssuedPermissions;
    use serde_json::json;

    struct HarnessOidcVerifier {
        caller_id: CallerId,
    }

    #[async_trait::async_trait]
    impl OidcVerifier for HarnessOidcVerifier {
        async fn verify(
            &self,
            _token: &BearerToken,
            account: &AudienceAccount,
        ) -> Result<UserJwtClaims, a2a_auth_callout::AuthCalloutError> {
            Ok(UserJwtClaims {
                sub: ExternalSubject::new("harness-sub").expect("fixture sub"),
                aud: account.clone(),
                data: SpiceDbPrincipal(json!({"spicedb_subject": "harness-sub"})),
                nats_permissions: IssuedPermissions::default_for_caller(&self.caller_id),
                caller_id: self.caller_id.clone(),
            })
        }
    }

    let caller = CallerId::new(caller_id).expect("harness caller_id");
    let oidc: Arc<dyn OidcVerifier> = Arc::new(HarnessOidcVerifier { caller_id: caller });
    let resolver: Arc<dyn a2a_auth_callout::AccountResolver> =
        Arc::new(StaticAccountResolver::new(["tenant-harness".to_string()]));
    CalloutDispatcher::new(CalloutDispatcherConfig {
        signing_key: SigningKey::from_secret(b"bridge-harness-callout-secret"),
        user_jwt_ttl: Duration::from_secs(60),
        account_resolver: resolver,
        oidc: Some(oidc),
        mtls: None,
        api_key: None,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use a2a_auth_callout::caller_id_from_minted_jwt;

    use super::*;
    use crate::auth::{AuthCalloutClient, AuthCalloutJsonMintClient};
    use crate::identity::CallerHttpsAuth;

    #[tokio::test]
    async fn harness_mint_wire_returns_deterministic_caller_id_jwt() {
        let tenant = BridgeTenantAccount::new("tenant-harness").unwrap();
        let dispatcher = Arc::new(harness_callout_dispatcher("bridge-harness-caller"));
        let wire = Arc::new(InProcessCalloutDispatcherMintWire::new(dispatcher, tenant.clone()));
        let client = AuthCalloutJsonMintClient::with_tenant_account(
            wire,
            "a2a.bridge.auth.callout.request",
            Some(tenant),
        );
        let jwt = client
            .mint(&CallerHttpsAuth::new("Bearer fixture-token"))
            .await
            .expect("harness mint");
        let caller = caller_id_from_minted_jwt(jwt.as_str()).expect("caller_id claim");
        assert_eq!(caller.as_str(), "bridge-harness-caller");
    }
}
