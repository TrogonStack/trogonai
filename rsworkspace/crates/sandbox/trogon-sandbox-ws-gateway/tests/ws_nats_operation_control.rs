mod support;

use std::sync::Arc;

use futures_util::StreamExt;
use serde_json::{Value, json};
use support::{TestResult, WAIT, api_key, open_socket, read_next, send};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CancelOperationRequest, CancelOperationResponse,
    CommandLaunchOrigin, CreateCancellationSupport, CreatedLeaseCompensation, ExecutionId, OperationCapabilities,
    RunCommandResponse, RunCommandStartedResponse, TenantId,
};
use trogon_sandbox_protocol::subjects::{OPERATION_CANCEL_REQUEST, OPERATION_CAPABILITIES_REQUEST};
use trogon_sandbox_ws_gateway::{CapabilityRegistry, NatsObservability, NatsSandboxBackend, ObservabilityConfig};

const LAUNCH_SUBJECT: &str = "sandbox.lease.run_command.owned-launch-lease";
const CANCEL_LAUNCH_SUBJECT: &str = "sandbox.lease.cancel_command_launch.owned-launch-lease";

fn backend(client: &async_nats::Client) -> Arc<NatsSandboxBackend> {
    Arc::new(NatsSandboxBackend::new(
        client.clone(),
        CapabilityRegistry::new(),
        NatsObservability::new(client.clone(), ObservabilityConfig::default()),
    ))
}

async fn next_request(subscription: &mut async_nats::Subscriber) -> TestResult<async_nats::Message> {
    tokio::time::timeout(WAIT, subscription.next())
        .await?
        .ok_or_else(|| "subscription closed".into())
}

fn assert_bearer(message: &async_nats::Message, key: &str) {
    let sent = message
        .headers
        .as_ref()
        .and_then(|headers| headers.get("Authorization"))
        .map(|value| value.as_str());
    assert_eq!(sent, Some(format!("Bearer {key}").as_str()));
}

async fn answer<T: serde::Serialize>(
    client: &async_nats::Client,
    message: &async_nats::Message,
    response: &T,
) -> TestResult {
    let reply = message.reply.clone().ok_or("the request has no reply subject")?;
    client.publish(reply, serde_json::to_vec(response)?.into()).await?;
    Ok(())
}

#[tokio::test]
async fn compensation_and_the_session_bearer_reach_the_nats_boundary() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let mut capabilities = client.subscribe(OPERATION_CAPABILITIES_REQUEST).await?;
    let mut cancellations = client.subscribe(OPERATION_CANCEL_REQUEST).await?;
    client.flush().await?;
    let key = api_key("operation-wire");
    let mut socket = open_socket(backend(&client), &key, "owner").await?;

    send(&mut socket, "cap", "sandbox_operation_capabilities", json!({})).await?;
    let request = next_request(&mut capabilities).await?;
    assert_bearer(&request, &key);
    answer(
        &client,
        &request,
        &OperationCapabilities {
            supports_create_cancellation: CreateCancellationSupport::Supported,
        },
    )
    .await?;
    assert_eq!(
        read_next(&mut socket).await?["result"],
        json!({"supports_create_cancellation": true})
    );

    send(
        &mut socket,
        "cancel",
        "sandbox_cancel_operation",
        json!({"operation_id": "owned-operation", "compensate_created": true}),
    )
    .await?;
    let request = next_request(&mut cancellations).await?;
    assert_bearer(&request, &key);
    let decoded: CancelOperationRequest = serde_json::from_slice(&request.payload)?;
    assert_eq!(decoded.tenant, TenantId::new("owner")?);
    assert_eq!(decoded.operation_id.as_str(), "owned-operation");
    assert_eq!(decoded.compensate_created, CreatedLeaseCompensation::Compensate);
    answer(&client, &request, &CancelOperationResponse::Canceled).await?;
    let response = read_next(&mut socket).await?;
    assert_eq!(response["id"], "cancel");
    assert_eq!(response["result"], json!({"outcome": "canceled"}));
    socket.close(None).await?;
    Ok(())
}

#[tokio::test]
async fn launch_cancellation_reaches_nats_while_the_start_reply_is_withheld() -> TestResult {
    let server = JetStreamTestServer::start().await;
    let client = server.client().await;
    let mut launches = client.subscribe(LAUNCH_SUBJECT).await?;
    let mut cancellations = client.subscribe(CANCEL_LAUNCH_SUBJECT).await?;
    client.flush().await?;
    let key = api_key("launch-wire");
    let mut socket = open_socket(backend(&client), &key, "owner").await?;

    send(
        &mut socket,
        "unacknowledged-start",
        "sandbox_run_command",
        json!({
            "lease_id": "owned-launch-lease", "command": ["sleep", "60"],
            "detached": true, "launch_key": "retained-before-start",
            "require_launch_cancellation": true
        }),
    )
    .await?;
    let launch = next_request(&mut launches).await?;
    assert_bearer(&launch, &key);

    send(
        &mut socket,
        "cancel-before-start-reply",
        "sandbox_cancel_command_launch",
        json!({"lease_id": "owned-launch-lease", "launch_key": "retained-before-start"}),
    )
    .await?;
    let cancellation = next_request(&mut cancellations).await?;
    assert_bearer(&cancellation, &key);
    let decoded: CancelCommandLaunchRequest = serde_json::from_slice(&cancellation.payload)?;
    assert_eq!(decoded.lease_id.as_str(), "owned-launch-lease");
    assert_eq!(decoded.launch_key.as_str(), "retained-before-start");
    answer(
        &client,
        &cancellation,
        &CancelCommandLaunchResponse::Canceled {
            execution_id: Some(ExecutionId::new("owned-execution")),
        },
    )
    .await?;
    let canceled = read_next(&mut socket).await?;
    assert_eq!(canceled["id"], "cancel-before-start-reply");
    assert_eq!(
        canceled["result"],
        json!({"outcome": "canceled", "execution_id": "owned-execution"})
    );

    answer(
        &client,
        &launch,
        &RunCommandResponse::Started(RunCommandStartedResponse {
            execution_id: ExecutionId::new("owned-execution"),
            origin: CommandLaunchOrigin::Created,
        }),
    )
    .await?;
    let started: Value = read_next(&mut socket).await?;
    assert_eq!(started["id"], "unacknowledged-start");
    assert_eq!(started["result"]["origin"], "created");
    assert_eq!(started["result"]["execution_id"], "owned-execution");
    socket.close(None).await?;
    Ok(())
}
