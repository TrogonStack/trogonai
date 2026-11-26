//! Answers the internal mint exchange: a bridge sends a [`BridgeMintRequest`]
//! and the callout runs it through the same dispatcher, and so the same
//! credential verification, as a server authorization request.

use tracing::warn;
use trogon_std::env::ReadEnv;

use crate::admission_keys::CalloutXkey;
use crate::bridge_mint::{BridgeMintRefusal, BridgeMintReply, BridgeMintRequest, BridgeMintResponse};
use crate::constants::{BRIDGE_MINT_SUBJECT_VAR, DEFAULT_BRIDGE_MINT_SUBJECT};
use crate::denial_category::DenialCategory;
use crate::dispatcher::AuthDispatcher;
use crate::error::{AuthCalloutError, CredentialError};
use crate::jwt::MintedUserJwt;
use crate::wire::ServerAuthRequestClaims;

/// The subject the callout answers bridge mint requests on:
/// `AUTH_CALLOUT_MINT_SUBJECT` when it names one, the bridge's default
/// otherwise. A blank value counts as unset, as it does for the bridge.
pub fn bridge_mint_subject(env: &impl ReadEnv) -> String {
    ReadEnv::var(env, BRIDGE_MINT_SUBJECT_VAR)
        .ok()
        .map(|subject| subject.trim().to_owned())
        .filter(|subject| !subject.is_empty())
        .unwrap_or_else(|| DEFAULT_BRIDGE_MINT_SUBJECT.to_owned())
}

/// Turns a bridge mint request into the synthetic authorization request the
/// dispatcher reads, and dispatches it. Shared by the responder and by the
/// bridge's in-process mint, so both mint exactly what the dispatcher grants.
pub async fn dispatch_bridge_mint<D: AuthDispatcher + ?Sized>(
    dispatcher: &D,
    callout_xkey: Option<&CalloutXkey>,
    request: BridgeMintRequest,
) -> Result<MintedUserJwt, AuthCalloutError> {
    let claims = ServerAuthRequestClaims::from_bridge_mint(request, callout_xkey)?;
    dispatcher.dispatch(claims).await
}

/// A request over the bus carries its caller's credential only sealed to the
/// callout xkey. Anything else would let the requester name an identity it
/// never proved, such as a client certificate PEM, which is public.
fn require_sealed_credential(request: &BridgeMintRequest) -> Result<(), CredentialError> {
    if request.sealed_credential.is_none() {
        return Err(CredentialError::InvalidRequest(
            "bridge mint request carries no sealed credential".into(),
        ));
    }
    Ok(())
}

pub struct BridgeMintResponder<D> {
    dispatcher: D,
    callout_xkey: Option<CalloutXkey>,
}

impl<D: AuthDispatcher> BridgeMintResponder<D> {
    pub fn new(dispatcher: D, callout_xkey: Option<CalloutXkey>) -> Self {
        Self {
            dispatcher,
            callout_xkey,
        }
    }

    /// The answer to one request payload. Every failure is a refusal that
    /// names only the opaque denial category; the typed error stays in the
    /// log.
    pub async fn respond(&self, payload: &[u8]) -> BridgeMintReply {
        match self.mint(payload).await {
            Ok(user_jwt) => BridgeMintReply::Minted(BridgeMintResponse {
                user_jwt: user_jwt.as_str().to_owned(),
            }),
            Err(error) => {
                let category = DenialCategory::from_auth_callout_error(&error).as_str();
                warn!(error = %error, category, "bridge mint refused");
                BridgeMintReply::Refused(BridgeMintRefusal::new(category))
            }
        }
    }

    async fn mint(&self, payload: &[u8]) -> Result<MintedUserJwt, AuthCalloutError> {
        let request: BridgeMintRequest = serde_json::from_slice(payload).map_err(AuthCalloutError::Deserialize)?;
        require_sealed_credential(&request)?;
        dispatch_bridge_mint(&self.dispatcher, self.callout_xkey.as_ref(), request).await
    }
}

#[cfg(test)]
mod tests;
