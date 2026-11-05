use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use a2a_auth_callout::dispatcher::AuthDispatcher;
use a2a_auth_callout::wire::ServerAuthRequestClaims;
use a2a_auth_callout::{AuthCalloutError, BridgeMintRequest, BridgeMintResponse, CalloutXkey};
use async_trait::async_trait;

use super::{AuthMintWire, BytesPayload};
use crate::error::BridgeError;

#[cfg(test)]
use a2a_auth_callout::admission_keys::{AccountSigningKey, TenantAccount, TenantSigningKeys};
#[cfg(test)]
use a2a_auth_callout::credentials::oidc::{BearerToken, OidcVerifier};
#[cfg(test)]
use a2a_auth_callout::dispatcher::{CalloutDispatcher, CalloutDispatcherConfig};
#[cfg(test)]
use a2a_auth_callout::jwt::{AudienceAccount, CallerId, ExternalSubject, SpiceDbPrincipal, UserJwtClaims};
#[cfg(test)]
use a2a_auth_callout::permissions::IssuedPermissions;
#[cfg(test)]
use a2a_auth_callout::signing_key_source::KeyVersion;
#[cfg(test)]
use serde_json::json;
#[cfg(test)]
use std::time::Duration;

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
    callout_xkey: Option<CalloutXkey>,
    mint_count: Arc<AtomicUsize>,
}

impl InProcessCalloutDispatcherMintWire {
    pub fn new(
        dispatcher: Arc<dyn AuthDispatcher>,
        tenant: BridgeTenantAccount,
        callout_xkey: Option<CalloutXkey>,
    ) -> Self {
        Self {
            dispatcher,
            tenant,
            callout_xkey,
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
        let claims =
            ServerAuthRequestClaims::from_bridge_mint(request, self.callout_xkey.as_ref()).map_err(|e| match e {
                AuthCalloutError::BridgeCredential(refused) => BridgeError::MintCredential(refused),
                other => BridgeError::Mint(other.to_string()),
            })?;
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
pub(crate) fn harness_callout_xkey() -> CalloutXkey {
    CalloutXkey::from_seed(&nkeys::XKey::new().seed().expect("harness xkey seed")).expect("harness callout xkey")
}

#[cfg(test)]
pub(crate) fn harness_callout_dispatcher(caller_id: &str) -> (Arc<CalloutDispatcher>, BridgeTenantAccount) {
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
                kid: KeyVersion::new("pending").expect("fixture kid"),
                sub: ExternalSubject::new("harness-sub").expect("fixture sub"),
                aud: account.clone(),
                data: SpiceDbPrincipal(json!({"spicedb_subject": "harness-sub"})),
                nats_permissions: IssuedPermissions::default_for_caller(&self.caller_id),
                caller_id: self.caller_id.clone(),
                principal_kind: trogon_identity_types::PrincipalKind::Person,
            })
        }
    }

    let caller = CallerId::new(caller_id).expect("harness caller_id");
    let oidc: Arc<dyn OidcVerifier> = Arc::new(HarnessOidcVerifier { caller_id: caller });
    let tenant = TenantAccount::parse(&nkeys::KeyPair::new_account().public_key()).expect("harness tenant account");
    let signing_key =
        AccountSigningKey::from_seed(&nkeys::KeyPair::new_account().seed().expect("harness signing seed"))
            .expect("harness signing key");
    let bridge_tenant = BridgeTenantAccount::new(tenant.as_str()).expect("harness bridge tenant");
    let tenant_signing_keys = TenantSigningKeys::new([(tenant, signing_key)]).expect("harness tenant signing keys");
    let dispatcher = CalloutDispatcher::new(CalloutDispatcherConfig {
        tenant_signing_keys,
        user_jwt_ttl: Duration::from_secs(60),
        a2a_prefix: a2a_nats::A2aPrefix::new(a2a_nats::DEFAULT_A2A_PREFIX).expect("harness prefix"),
        oidc: Some(oidc),
        mtls: None,
        api_key: None,
    });
    (Arc::new(dispatcher), bridge_tenant)
}

#[cfg(test)]
mod tests;
