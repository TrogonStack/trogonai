use async_nats::Message;
use tracing::warn;
use trogon_sandbox_ssh_gateway::{SshAttachRequest, SshDetachRequest};

use crate::K8sVerb;
use crate::inbound::Inbound;
use crate::verb_context::VerbContext;

pub(crate) async fn ssh_attach(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<SshAttachRequest>::decode(&msg, K8sVerb::SshAttach) else {
        return;
    };
    let request = inbound.request;
    let Some(response) = ctx.bridge.attach(&request).await else {
        return;
    };
    if response.ok {
        let installed = ctx
            .provider
            .ssh_attach_for_lease(&request.lease_id, request.session_id.as_str(), &request.public_key)
            .await;
        if let Some(Err(error)) = installed {
            warn!(%error, "ssh session key was not installed; the stream bridge still serves the session");
        }
    }
    ctx.reply(inbound.reply, &response).await;
}

pub(crate) async fn ssh_detach(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<SshDetachRequest>::decode(&msg, K8sVerb::SshDetach) else {
        return;
    };
    let request = inbound.request;
    let Some(response) = ctx.bridge.detach(&request).await else {
        return;
    };
    if response.ok {
        let removed = ctx
            .provider
            .ssh_detach_for_lease(&request.lease_id, request.session_id.as_str())
            .await;
        if let Some(Err(error)) = removed {
            warn!(%error, "ssh session key was not removed");
        }
    }
    ctx.reply(inbound.reply, &response).await;
}
