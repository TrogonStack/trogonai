mod config;

use std::error::Error;
use std::future::Future;

use rmcp::service::{RoleClient, RoleServer};
use rmcp::transport::Transport;
#[cfg(not(coverage))]
use rmcp::transport::async_rw::AsyncRwTransport;
#[cfg(not(coverage))]
use rmcp::transport::stdio;
use tracing::{error, info};

type BoxError = Box<dyn Error + Send + Sync>;

#[cfg(not(coverage))]
use trogon_std::{env::SystemEnv, fs::SystemFs, signal::shutdown_signal};
#[cfg(not(coverage))]
use trogon_telemetry::{ResourceAttribute, ServiceName};

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let config = config::base_config(&trogon_std::CliArgs::<config::Args>::new(), &SystemEnv)?;
    let config::BridgeConfig {
        mcp,
        client_id,
        server_id,
    } = config;
    let mcp = mcp_nats::apply_timeout_overrides(mcp, &SystemEnv);
    trogon_telemetry::init_logger(
        ServiceName::McpNatsStdio,
        [ResourceAttribute::mcp_prefix(mcp.prefix_str())],
        &SystemEnv,
        &SystemFs,
    );

    info!(
        mcp_prefix = %mcp.prefix_str(),
        client_id = %client_id,
        server_id = %server_id,
        "MCP bridge starting"
    );

    let nats_connect_timeout = mcp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = mcp_nats::nats::connect(mcp.nats(), nats_connect_timeout).await?;
    let remote = mcp_nats::client::connect(nats_client, &mcp, client_id, server_id).await?;
    let (stdin, stdout) = stdio();
    let local = AsyncRwTransport::<RoleServer, _, _>::new_server(stdin, stdout);

    let result = run_bridge(local, remote, shutdown_signal()).await;

    match &result {
        Ok(()) => info!("MCP bridge stopped"),
        Err(error) => error!(error = %error, "MCP bridge stopped with error"),
    }

    if let Err(error) = trogon_telemetry::shutdown_otel() {
        error!(error = %error, "OpenTelemetry shutdown failed");
    }

    result
}

#[cfg(coverage)]
fn main() {}

async fn run_bridge<L, R, S>(mut local: L, mut remote: R, shutdown_signal: S) -> Result<(), BoxError>
where
    L: Transport<RoleServer>,
    R: Transport<RoleClient>,
    S: Future<Output = ()> + Send,
{
    info!("MCP bridge running on stdio with NATS client proxy");

    let result = {
        tokio::pin!(shutdown_signal);
        loop {
            tokio::select! {
                local_message = local.receive() => {
                    let Some(local_message) = local_message else {
                        info!("MCP bridge shutting down (stdio closed)");
                        break Ok(());
                    };
                    if let Err(error) = remote.send(local_message).await {
                        break Err(boxed_error(error));
                    }
                }
                remote_message = remote.receive() => {
                    let Some(remote_message) = remote_message else {
                        info!("MCP bridge shutting down (NATS transport closed)");
                        break Ok(());
                    };
                    if let Err(error) = local.send(remote_message).await {
                        break Err(boxed_error(error));
                    }
                }
                _ = &mut shutdown_signal => {
                    info!("MCP bridge shutting down (signal received)");
                    break Ok(());
                }
            }
        }
    };

    if let Err(error) = local.close().await {
        error!(error = %error, "Failed to close stdio MCP transport");
    }
    if let Err(error) = remote.close().await {
        error!(error = %error, "Failed to close NATS MCP transport");
    }

    result
}

fn boxed_error<E>(error: E) -> BoxError
where
    E: Error + Send + Sync + 'static,
{
    Box::new(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mcp_nats::{ClientJsonRpcMessage, ServerJsonRpcMessage};
    use rmcp::model::{ClientNotification, RequestId, ServerResult};
    use rmcp::service::{RxJsonRpcMessage, ServiceRole, TxJsonRpcMessage};
    use std::fmt::{Display, Formatter};
    use std::marker::PhantomData;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::sync::{mpsc, oneshot};

    struct ChannelTransport<R>
    where
        R: ServiceRole,
    {
        inbound_rx: mpsc::Receiver<RxJsonRpcMessage<R>>,
        sent_tx: mpsc::Sender<TxJsonRpcMessage<R>>,
        closed: Arc<AtomicUsize>,
    }

    type ChannelTransportParts<R> = (
        ChannelTransport<R>,
        mpsc::Sender<RxJsonRpcMessage<R>>,
        mpsc::Receiver<TxJsonRpcMessage<R>>,
    );

    type ChannelTransportPartsWithClose<R> = (ChannelTransportParts<R>, Arc<AtomicUsize>);

    impl<R> ChannelTransport<R>
    where
        R: ServiceRole,
    {
        fn new() -> ChannelTransportParts<R> {
            Self::new_with_close_counter().0
        }

        fn new_with_close_counter() -> ChannelTransportPartsWithClose<R> {
            let (inbound_tx, inbound_rx) = mpsc::channel(16);
            let (sent_tx, sent_rx) = mpsc::channel(16);
            let closed = Arc::new(AtomicUsize::new(0));
            (
                (
                    Self {
                        inbound_rx,
                        sent_tx,
                        closed: closed.clone(),
                    },
                    inbound_tx,
                    sent_rx,
                ),
                closed,
            )
        }
    }

    impl<R> Transport<R> for ChannelTransport<R>
    where
        R: ServiceRole + 'static,
    {
        type Error = mpsc::error::SendError<TxJsonRpcMessage<R>>;

        fn send(
            &mut self,
            item: TxJsonRpcMessage<R>,
        ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
            let sent_tx = self.sent_tx.clone();
            async move { sent_tx.send(item).await }
        }

        async fn receive(&mut self) -> Option<RxJsonRpcMessage<R>> {
            self.inbound_rx.recv().await
        }

        async fn close(&mut self) -> Result<(), Self::Error> {
            self.closed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Debug)]
    struct CloseError;

    impl Display for CloseError {
        fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
            write!(f, "close failed")
        }
    }

    impl Error for CloseError {}

    struct FailingCloseTransport<R>
    where
        R: ServiceRole,
    {
        role: PhantomData<R>,
    }

    impl<R> FailingCloseTransport<R>
    where
        R: ServiceRole,
    {
        fn new() -> Self {
            Self { role: PhantomData }
        }
    }

    impl<R> Transport<R> for FailingCloseTransport<R>
    where
        R: ServiceRole + 'static,
    {
        type Error = CloseError;

        fn send(
            &mut self,
            _item: TxJsonRpcMessage<R>,
        ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
            std::future::ready(Ok(()))
        }

        async fn receive(&mut self) -> Option<RxJsonRpcMessage<R>> {
            None
        }

        async fn close(&mut self) -> Result<(), Self::Error> {
            Err(CloseError)
        }
    }

    fn as_json<T: serde::Serialize>(value: &T) -> serde_json::Value {
        serde_json::to_value(value).unwrap()
    }

    #[tokio::test]
    async fn forwards_local_client_messages_to_remote_server() {
        let (local, local_inbound, _local_sent) = ChannelTransport::<RoleServer>::new();
        let (remote, _remote_inbound, mut remote_sent) = ChannelTransport::<RoleClient>::new();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let bridge = tokio::spawn(run_bridge(local, remote, async {
            let _ = shutdown_rx.await;
        }));
        let message =
            ClientJsonRpcMessage::notification(ClientNotification::InitializedNotification(Default::default()));

        local_inbound.send(message.clone()).await.unwrap();

        assert_eq!(as_json(&remote_sent.recv().await.unwrap()), as_json(&message));
        let _ = shutdown_tx.send(());
        bridge.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn forwards_remote_server_messages_to_local_client() {
        let (local, _local_inbound, mut local_sent) = ChannelTransport::<RoleServer>::new();
        let (remote, remote_inbound, _remote_sent) = ChannelTransport::<RoleClient>::new();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let bridge = tokio::spawn(run_bridge(local, remote, async {
            let _ = shutdown_rx.await;
        }));
        let message = ServerJsonRpcMessage::response(ServerResult::empty(()), RequestId::Number(1));

        remote_inbound.send(message.clone()).await.unwrap();

        assert_eq!(as_json(&local_sent.recv().await.unwrap()), as_json(&message));
        let _ = shutdown_tx.send(());
        bridge.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn run_bridge_shuts_down_on_signal() {
        let (local, _local_inbound, _local_sent) = ChannelTransport::<RoleServer>::new();
        let (remote, _remote_inbound, _remote_sent) = ChannelTransport::<RoleClient>::new();

        let result = run_bridge(local, remote, std::future::ready(())).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_bridge_exits_when_remote_closes() {
        let (local, local_inbound, _local_sent) = ChannelTransport::<RoleServer>::new();
        let (remote, remote_inbound, _remote_sent) = ChannelTransport::<RoleClient>::new();
        drop(remote_inbound);

        let result = run_bridge(local, remote, std::future::pending::<()>()).await;

        assert!(result.is_ok());
        drop(local_inbound);
    }

    #[tokio::test]
    async fn run_bridge_exits_when_local_closes() {
        let (local, local_inbound, _local_sent) = ChannelTransport::<RoleServer>::new();
        let (remote, remote_inbound, _remote_sent) = ChannelTransport::<RoleClient>::new();
        drop(local_inbound);

        let result = run_bridge(local, remote, std::future::pending::<()>()).await;

        assert!(result.is_ok());
        drop(remote_inbound);
    }

    #[tokio::test]
    async fn run_bridge_returns_error_when_remote_send_fails() {
        let (local, local_inbound, _local_sent) = ChannelTransport::<RoleServer>::new();
        let (remote, _remote_inbound, remote_sent) = ChannelTransport::<RoleClient>::new();
        drop(remote_sent);
        let message =
            ClientJsonRpcMessage::notification(ClientNotification::InitializedNotification(Default::default()));
        local_inbound.send(message).await.unwrap();

        let result = run_bridge(local, remote, std::future::pending::<()>()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_bridge_closes_transports_when_remote_send_fails() {
        let ((local, local_inbound, _local_sent), local_closed) =
            ChannelTransport::<RoleServer>::new_with_close_counter();
        let ((remote, _remote_inbound, remote_sent), remote_closed) =
            ChannelTransport::<RoleClient>::new_with_close_counter();
        drop(remote_sent);
        let message =
            ClientJsonRpcMessage::notification(ClientNotification::InitializedNotification(Default::default()));
        local_inbound.send(message).await.unwrap();

        let result = run_bridge(local, remote, std::future::pending::<()>()).await;

        assert!(result.is_err());
        assert_eq!(local_closed.load(Ordering::SeqCst), 1);
        assert_eq!(remote_closed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn run_bridge_returns_error_when_local_send_fails() {
        let (local, _local_inbound, local_sent) = ChannelTransport::<RoleServer>::new();
        let (remote, remote_inbound, _remote_sent) = ChannelTransport::<RoleClient>::new();
        drop(local_sent);
        let message = ServerJsonRpcMessage::response(ServerResult::empty(()), RequestId::Number(1));
        remote_inbound.send(message).await.unwrap();

        let result = run_bridge(local, remote, std::future::pending::<()>()).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn run_bridge_closes_transports_when_local_send_fails() {
        let ((local, _local_inbound, local_sent), local_closed) =
            ChannelTransport::<RoleServer>::new_with_close_counter();
        let ((remote, remote_inbound, _remote_sent), remote_closed) =
            ChannelTransport::<RoleClient>::new_with_close_counter();
        drop(local_sent);
        let message = ServerJsonRpcMessage::response(ServerResult::empty(()), RequestId::Number(1));
        remote_inbound.send(message).await.unwrap();

        let result = run_bridge(local, remote, std::future::pending::<()>()).await;

        assert!(result.is_err());
        assert_eq!(local_closed.load(Ordering::SeqCst), 1);
        assert_eq!(remote_closed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn run_bridge_continues_when_close_fails() {
        let local = FailingCloseTransport::<RoleServer>::new();
        let remote = FailingCloseTransport::<RoleClient>::new();

        let result = run_bridge(local, remote, std::future::pending::<()>()).await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn failing_close_transport_send_is_noop() {
        let mut local = FailingCloseTransport::<RoleServer>::new();
        let mut remote = FailingCloseTransport::<RoleClient>::new();
        let server_message = ServerJsonRpcMessage::response(ServerResult::empty(()), RequestId::Number(2));
        let client_message =
            ClientJsonRpcMessage::notification(ClientNotification::InitializedNotification(Default::default()));

        assert!(local.send(server_message).await.is_ok());
        assert!(remote.send(client_message).await.is_ok());
    }

    #[test]
    fn close_error_display_is_specific() {
        assert_eq!(CloseError.to_string(), "close failed");
    }

    #[test]
    #[cfg(coverage)]
    fn coverage_main_stub_is_callable() {
        main();
    }
}
