mod common;
mod lease_support;

use common::{TestResult, client_replying, tenant};
use lease_support::sample_operation;
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{
    CancelOperationResponse, CreateCancellationSupport, GetOperationRequest, GetOperationResponse, LeaseError,
    OperationCapabilities, OperationId, OperationState,
};

#[tokio::test]
async fn get_operation_returns_the_record_and_maps_not_found() -> TestResult {
    let operation_id = OperationId::new("op-1")?;
    let found = GetOperationResponse::Found {
        operation: Box::new(sample_operation("op-1")?),
    };
    let (client, log) = client_replying(&found)?;
    let record = client.get_operation(tenant()?, &operation_id).await?;
    assert_eq!(record.id, operation_id);
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.operation.get");
    let request: GetOperationRequest = serde_json::from_slice(&sent.payload)?;
    assert_eq!(request.operation_id, operation_id);

    let (client, _) = client_replying(&GetOperationResponse::NotFound)?;
    let out = client.get_operation(tenant()?, &operation_id).await;
    assert!(matches!(
        out,
        Err(ClientError::Denied(LeaseError::OperationNotFound { .. }))
    ));

    let denied = GetOperationResponse::Denied(LeaseError::Backend { message: "down".into() });
    let (client, _) = client_replying(&denied)?;
    let out = client.get_operation(tenant()?, &operation_id).await;
    assert!(matches!(out, Err(ClientError::Denied(LeaseError::Backend { .. }))));
    Ok(())
}

#[tokio::test]
async fn operation_capabilities_returns_the_capabilities_and_surfaces_denial() -> TestResult {
    let capabilities = OperationCapabilities {
        supports_create_cancellation: CreateCancellationSupport::Supported,
    };
    let (client, log) = client_replying(&capabilities)?;
    assert_eq!(client.operation_capabilities(tenant()?).await?, capabilities);
    assert_eq!(log.only()?.subject, "sandbox.operation.capabilities");

    let denied = CancelOperationResponse::Denied(LeaseError::Unauthorized { reason: "no".into() });
    let (client, _) = client_replying(&denied)?;
    let out = client.operation_capabilities(tenant()?).await;
    assert!(matches!(out, Err(ClientError::Denied(LeaseError::Unauthorized { .. }))));

    let (client, _) = client_replying(&"garbage")?;
    let out = client.operation_capabilities(tenant()?).await;
    assert!(matches!(out, Err(ClientError::Transport(_))));
    Ok(())
}

#[tokio::test]
async fn cancel_operation_keeps_created_leases_and_maps_answers() -> TestResult {
    let (client, log) = client_replying(&CancelOperationResponse::Canceled)?;
    let out = client.cancel_operation(tenant()?, OperationId::new("op-1")?).await?;
    assert_eq!(out, CancelOperationResponse::Canceled);
    let sent = log.only()?;
    assert_eq!(sent.subject, "sandbox.operation.cancel");
    assert_eq!(sent.json()?["compensate_created"], false);

    let done = CancelOperationResponse::AlreadyCompleted {
        state: OperationState::Succeeded,
    };
    let (client, _) = client_replying(&done)?;
    let out = client.cancel_operation(tenant()?, OperationId::new("op-1")?).await?;
    assert_eq!(out, done);

    let denied = CancelOperationResponse::Denied(LeaseError::Backend { message: "down".into() });
    let (client, _) = client_replying(&denied)?;
    let out = client.cancel_operation(tenant()?, OperationId::new("op-1")?).await;
    assert!(matches!(out, Err(ClientError::Denied(LeaseError::Backend { .. }))));
    Ok(())
}
