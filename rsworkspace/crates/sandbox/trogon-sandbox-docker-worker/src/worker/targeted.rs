use super::*;
use trogon_sandbox_contract::{ProviderCleanupRequest, ProviderCleanupResponse};

pub(super) async fn serve(
    client: &Client,
    docker: &Arc<DockerWorkerProvider>,
    active: &ActiveLeases,
    grants: &Arc<RedispatchGuard>,
    terminals: &Arc<DockerTerminals>,
    current: &Arc<AtomicU32>,
    cancel: &CancellationToken,
) -> anyhow::Result<()> {
    let Some(ledger) = docker.dispatch_ledger() else {
        return Ok(());
    };
    let mut dispatch = subscribe_verified(
        client,
        trogon_sandbox_protocol::subjects::worker_dispatch(
            trogon_sandbox_contract::ProviderKind::LocalDocker,
            ledger.worker_id(),
        ),
    )
    .await?;
    let mut cleanup = subscribe_verified_context(
        client,
        trogon_sandbox_protocol::subjects::worker_cleanup(
            trogon_sandbox_contract::ProviderKind::LocalDocker,
            ledger.worker_id(),
        ),
    )
    .await?;
    let worker_id = ledger.worker_id().clone();
    let (client, docker, active, grants, terminals, current, cancel) = (
        client.clone(),
        docker.clone(),
        active.clone(),
        grants.clone(),
        terminals.clone(),
        current.clone(),
        cancel.clone(),
    );
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = cancel.cancelled() => break,
                message = dispatch.next() => {
                    let Some(message) = message else { break; };
                    let (client, docker, active, current, worker_id) = (client.clone(), docker.clone(), active.clone(), current.clone(), worker_id.clone());
                    tokio::spawn(async move {
                        current.fetch_add(1, Ordering::Relaxed);
                        let result = async {
                            let bytes = trogon_sandbox_protocol::nats_transfer::receive(&client, &message).await.map_err(ProviderError::backend)?;
                            let payload: ProviderDispatchPayload = trogon_sandbox_protocol::decode_verified(&bytes, Some(&worker_id)).map_err(ProviderError::backend)?;
                            let receipt = docker.dispatch_targeted(&payload).await.map_err(ProviderError::backend)?;
                            active.lock().await.entry(payload.lease_id.clone()).or_insert_with(CancellationToken::new);
                            Ok::<_, ProviderError>(receipt)
                        }.await;
                        current.fetch_sub(1, Ordering::Relaxed);
                        if let Some(reply) = message.reply {
                            let bytes = match result {
                                Ok(receipt) => serde_json::to_vec(&receipt),
                                Err(error) => serde_json::to_vec(&trogon_sandbox_protocol::dispatch_failure_reply(&error)),
                            };
                            if let Ok(bytes) = bytes { let _ = client.publish(reply, bytes.into()).await; }
                        }
                    });
                }
                verified = cleanup.next() => {
                    let Some(verified) = verified else { break; };
                    let (client, docker, active, grants, terminals) = (client.clone(), docker.clone(), active.clone(), grants.clone(), terminals.clone());
                    tokio::spawn(async move {
                        let message = verified.message;
                        let Some(reply) = message.reply.clone() else { return; };
                        let Ok(bytes) = trogon_sandbox_protocol::nats_transfer::receive(&client, &message).await else { return; };
                        let Ok(request) = serde_json::from_slice::<ProviderCleanupRequest>(&bytes) else { return; };
                        let response = match verified.scope {
                            Some(scope) if scope.matches(message.subject.as_str(), &bytes)
                                && scope.scope().names_assignment(&request) => {
                                    docker.cleanup_targeted(&request, &scope.scope().tenant.clone()).await
                                }
                            _ => ProviderCleanupResponse::Failed { identity: request.identity.clone(), message: "Docker cleanup requires exact verified assignment authority".into() },
                        };
                        if matches!(response, ProviderCleanupResponse::Cleaned { .. }) {
                            terminals.close_lease(&client, &request.identity.lease_id).await;
                            if let Some(token) = active.lock().await.remove(&request.identity.lease_id) { token.cancel(); }
                            grants.forget(&request.identity.lease_id);
                        }
                        if let Ok(bytes) = serde_json::to_vec(&response) { let _ = client.publish(reply, bytes.into()).await; }
                    });
                }
            }
        }
    });
    Ok(())
}
