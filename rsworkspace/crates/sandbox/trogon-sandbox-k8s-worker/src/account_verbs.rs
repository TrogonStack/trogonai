use async_nats::Message;
use trogon_sandbox_contract::{
    CreateSandboxGroupRequest, CreateSandboxGroupResponse, CreateSandboxUserRequest, CreateSandboxUserResponse,
    LeaseError, ModifyGroupMembershipRequest, ModifyGroupMembershipResponse,
};

use crate::K8sVerb;
use crate::inbound::Inbound;
use crate::verb_context::VerbContext;

fn backend(error: impl std::fmt::Display) -> LeaseError {
    LeaseError::Backend {
        message: error.to_string(),
    }
}

pub(crate) async fn create_user(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<CreateSandboxUserRequest>::decode(&msg, K8sVerb::CreateUser) else {
        return;
    };
    let request = inbound.request;
    let result = ctx
        .provider
        .create_user_for_lease(&request.lease_id, &request.username)
        .await;
    if let Some(result) = result {
        let response = result.unwrap_or_else(|error| CreateSandboxUserResponse::Denied(backend(error)));
        ctx.reply(inbound.reply, &response).await;
    }
}

pub(crate) async fn create_group(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<CreateSandboxGroupRequest>::decode(&msg, K8sVerb::CreateGroup) else {
        return;
    };
    let request = inbound.request;
    let result = ctx
        .provider
        .create_group_for_lease(&request.lease_id, &request.groupname)
        .await;
    if let Some(result) = result {
        let response = result.unwrap_or_else(|error| CreateSandboxGroupResponse::Denied(backend(error)));
        ctx.reply(inbound.reply, &response).await;
    }
}

pub(crate) async fn modify_group_membership(ctx: &VerbContext, msg: Message) {
    let Some(inbound) = Inbound::<ModifyGroupMembershipRequest>::decode(&msg, K8sVerb::ModifyGroupMembership) else {
        return;
    };
    let request = inbound.request;
    let result = ctx
        .provider
        .modify_group_membership_for_lease(&request.lease_id, &request.username, &request.groupname, request.action)
        .await;
    if let Some(result) = result {
        let response = result.unwrap_or_else(|error| ModifyGroupMembershipResponse::Denied(backend(error)));
        ctx.reply(inbound.reply, &response).await;
    }
}
