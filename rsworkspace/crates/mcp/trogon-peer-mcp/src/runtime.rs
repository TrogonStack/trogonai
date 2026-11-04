use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use grpc_nats_micro::ContentType;
use rmcp::ServiceExt;
use rmcp::service::RoleServer;
use rmcp::transport::IntoTransport;
use tokio_util::sync::CancellationToken;
use trogon_peer_nats::{PeerRequester, PeerRpcClient};

use crate::availability_observation::AvailabilityObservation;
use crate::backend::{
    AcknowledgePeerBatch, DetachPeer, EnrollPeer, HeartbeatPeer, PreparePeerBatch, ReadPeerMessageStatus,
    ReadPeerRoster, SendPeerMessage,
};
use crate::observed_transport::ObservedTransport;
use crate::peer_mcp::PeerMcp;
use crate::rpc_peer_backend::RpcPeerBackend;
use crate::runtime_config::RuntimeConfig;
use crate::runtime_error::RuntimeError;

pub async fn run_stdio<F: Future<Output = ()> + Send>(config: RuntimeConfig, shutdown: F) -> Result<(), RuntimeError> {
    let connection = trogon_nats::connect_once(config.nats(), config.nats().auth.clone(), Duration::from_secs(2));
    let client = tokio::time::timeout(Duration::from_secs(2), connection)
        .await
        .map_err(|_| RuntimeError::StartupDeadline)?
        .map_err(RuntimeError::Connect)?;
    let rpc = PeerRpcClient::new(
        PeerRequester::new(client.clone(), config.requester().clone()),
        ContentType::Protobuf,
    );
    let adapter =
        Arc::new(PeerMcp::new(RpcPeerBackend::new(rpc), config.connector_input()).map_err(RuntimeError::Connector)?);
    let result = serve_connector(
        adapter,
        crate::stdio_transport::native_stdio().map_err(RuntimeError::Stdio)?,
        config.availability(),
        shutdown,
    )
    .await;
    let _drained = tokio::time::timeout(Duration::from_secs(1), client.drain()).await;
    result
}

pub async fn serve_connector<B, T, E, A, F>(
    adapter: Arc<PeerMcp<B>>,
    transport: T,
    availability: AvailabilityObservation,
    shutdown: F,
) -> Result<(), RuntimeError>
where
    B: EnrollPeer
        + PreparePeerBatch
        + ReadPeerRoster
        + SendPeerMessage
        + ReadPeerMessageStatus
        + AcknowledgePeerBatch
        + HeartbeatPeer
        + DetachPeer,
    T: IntoTransport<RoleServer, E, A>,
    E: std::error::Error + Send + Sync + 'static,
    F: Future<Output = ()> + Send,
{
    tokio::pin!(shutdown);
    let cancellation = CancellationToken::new();
    let closed = CancellationToken::new();
    let transport = ObservedTransport::new(transport.into_transport(), closed.clone());
    let service = tokio::select! {
        biased;
        _ = &mut shutdown => return adapter.shutdown().await.map_err(RuntimeError::Connector),
        service = adapter.clone().serve_with_ct(transport, cancellation.clone()) => service.map_err(|error| RuntimeError::Initialize(Box::new(error)))?,
    };
    let waiting = service.waiting();
    tokio::pin!(waiting);
    let completed = {
        let heartbeat = async {
            adapter.run_heartbeat(availability).await;
            std::future::pending::<()>().await;
        };
        tokio::pin!(heartbeat);
        tokio::select! {
            biased;
            _ = &mut shutdown => None,
            _ = closed.cancelled() => None,
            result = &mut waiting => Some(result),
            _ = &mut heartbeat => None,
        }
    };
    cancellation.cancel();
    let detached = adapter.shutdown().await;
    let result = match completed {
        Some(result) => result.map_err(RuntimeError::Shutdown).map(|_| ()),
        None => tokio::time::timeout(Duration::from_secs(1), &mut waiting)
            .await
            .map_err(|_| RuntimeError::ShutdownDeadline)?
            .map_err(RuntimeError::Shutdown)
            .map(|_| ()),
    };
    detached.map_err(RuntimeError::Connector)?;
    result
}
