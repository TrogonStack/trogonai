use std::error::Error;
use std::sync::Arc;

use async_nats::Client;
use bytes::Bytes;
use trogon_sandbox_contract::{
    LeaseId, ProviderDispatchPayload, ProviderDispatchReply, ProviderKind, ProviderLease, SandboxId,
};
use trogon_sandbox_protocol::subjects::{evidence_subject, provider_dispatch};
use trogon_sandbox_protocol::{NativeOptionsError, ProviderError, decode_verified, nats_transfer};
use trogon_sandbox_protocol::{dispatch_failure_reply, options_for, reject_unknown_keys};

use super::command_task::ClosePolicy;
use super::handle_verb::handle_verb;
use super::lease_task::run_lease_task;
use super::state::State;
use super::verb::{ServedVerb, Verb};

/// What a handler of one message can fail with.
type RouteResult = Result<(), Box<dyn Error + Send + Sync>>;

/// Handles one message on the dispatch subject or on a held lease's verb subject.
pub(crate) async fn route(state: Arc<State>, client: &Client, msg: async_nats::Message) -> RouteResult {
    let subject = msg.subject.to_string();
    if subject == provider_dispatch(state.kind) {
        return handle_lease(&state, client, msg).await;
    }
    // sandbox.provider.<token>.<verb>.<lease_id>
    let tokens: Vec<&str> = subject.split('.').collect();
    // The holder subscription guarantees both tokens; an empty one names no verb and no lease.
    let verb = tokens.get(3).copied().unwrap_or_default();
    let lease_id = LeaseId::new(tokens.get(4).copied().unwrap_or_default());
    if verb == "cancel" {
        state.close_lease(client, &lease_id).await;
        return Ok(());
    }
    let Some(verb) = Verb::parse(verb) else {
        return Ok(());
    };
    // A worker that does not hold the lease stays silent: the verb is broadcast.
    let Some(lease) = state.lease(&lease_id) else {
        return Ok(());
    };
    let Some(reply) = msg.reply.clone() else {
        return Ok(());
    };
    let body = nats_transfer::receive(client, &msg).await?;
    let close_policy = match verb {
        Verb::Served(ServedVerb::RunCommand) => ClosePolicy::JoinResponse,
        _ => ClosePolicy::Abort,
    };
    let out = run_lease_task(&lease, close_policy, {
        let state = Arc::clone(&state);
        let lease = Arc::clone(&lease);
        let client = client.clone();
        async move {
            let out = match verb {
                Verb::Terminal => serde_json::to_vec(&state.terminals.handle(&client, &lease_id, &body).await)?,
                Verb::Served(served) => {
                    let out = handle_verb(&state, &lease, served, &body).await?;
                    if served == ServedVerb::FileOp {
                        lease.publish_watch_overflows(&client).await;
                    }
                    out
                }
                Verb::Unsupported(unsupported) => unsupported.reply()?,
            };
            Ok::<_, serde_json::Error>(out)
        }
    })
    .await;
    let Some(out) = out else { return Ok(()) };
    nats_transfer::respond(client, reply.as_str(), Bytes::from(out?)).await?;
    Ok(())
}

/// Grants a lease, or refuses a dispatch it cannot honor, and replies to the dispatcher.
async fn handle_lease(state: &State, client: &Client, msg: async_nats::Message) -> RouteResult {
    let Some(reply) = msg.reply.clone() else {
        return Ok(());
    };
    let reply_body = match decode_verified::<ProviderDispatchPayload>(&msg.payload, None) {
        Err(error) => ProviderDispatchReply::DispatchRejected {
            decode_error: error.to_string(),
        },
        Ok(payload) => match check_native(state.kind, payload.request.native.as_ref()) {
            Err(error) => dispatch_failure_reply(&ProviderError::from(error)),
            Ok(()) => match SandboxId::new(payload.lease_id.as_str()) {
                Err(error) => ProviderDispatchReply::DispatchRejected {
                    decode_error: error.to_string(),
                },
                Ok(sandbox_id) => {
                    state.grant(&payload.lease_id, payload.request.region.clone()).await?;
                    ProviderDispatchReply::Granted(ProviderLease {
                        epoch: None,
                        sandbox_id,
                        evidence_subject: evidence_subject(&payload.lease_id),
                        worker_id: Some(state.worker_id.clone()),
                        attempt_id: None,
                    })
                }
            },
        },
    };
    let body = serde_json::to_vec(&reply_body)?;
    client.publish(reply, body.into()).await?;
    Ok(())
}

/// The fake's only native option is a free-form `label`, enough to exercise
/// validation and forwarding without a vendor behind it.
pub(crate) fn check_native(kind: ProviderKind, native: Option<&serde_json::Value>) -> Result<(), NativeOptionsError> {
    let options = options_for(kind, native)?;
    reject_unknown_keys(&options, &["label"])
}
