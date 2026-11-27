use async_nats::Message;
use trogon_sandbox_contract::{GetSandboxDomainRequest, GetSandboxDomainResponse, TerminalRequest};

use crate::K8sVerb;
use crate::inbound::Inbound;
use crate::verb_context::VerbContext;

pub(crate) async fn terminal(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<TerminalRequest>::decode(&msg, K8sVerb::Terminal) else {
        return;
    };
    if !ctx.terminals.holds(&inbound.request.lease_id).await {
        return;
    }
    let response = ctx.terminals.handle(&msg.payload).await;
    ctx.reply(inbound.reply, &response).await;
}

/// Pods expose no per-lease ingress, so a held lease answers with a typed `Unsupported` rather than the silence
/// that would leave the caller waiting out its request timeout.
pub(crate) async fn get_domain(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<GetSandboxDomainRequest>::decode(&msg, K8sVerb::GetDomain) else {
        return;
    };
    if ctx.provider.holds_lease(&inbound.request.lease_id).await {
        ctx.reply(inbound.reply, &GetSandboxDomainResponse::Unsupported).await;
    }
}
