use async_nats::Message;
use trogon_sandbox_contract::{LeaseError, NetworkPolicyUpdateRequest, NetworkPolicyUpdateResponse};

use crate::K8sVerb;
use crate::inbound::Inbound;
use crate::policy_rules::ensure_policy_enforceable;
use crate::verb_context::VerbContext;

/// Applies a network policy to a running lease. A policy the worker cannot enforce is refused before anything is
/// pushed, so a running lease keeps its current policy instead of a silently weaker one.
pub(crate) async fn network_policy_update(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<NetworkPolicyUpdateRequest>::decode(&msg, K8sVerb::NetworkPolicyUpdate) else {
        return;
    };
    let request = inbound.request;
    if !ctx.provider.holds_lease(&request.lease_id).await {
        return;
    }
    let response = match ensure_policy_enforceable(Some(&request.network_policy)) {
        Err(error) => NetworkPolicyUpdateResponse::Denied(error),
        Ok(()) => {
            let pushed = ctx
                .provider
                .push_network_policy_for_lease(&request.lease_id, &request.network_policy)
                .await;
            match pushed {
                Ok(()) => NetworkPolicyUpdateResponse::Applied,
                Err(error) => NetworkPolicyUpdateResponse::Denied(LeaseError::Backend {
                    message: error.to_string(),
                }),
            }
        }
    };
    ctx.reply(inbound.reply, &response).await;
}
