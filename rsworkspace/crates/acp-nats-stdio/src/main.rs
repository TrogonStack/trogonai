#![cfg_attr(coverage, feature(coverage_attribute))]
#![cfg_attr(coverage, allow(dead_code, unused_imports))]
mod config;

use acp_nats::{StdJsonSerialize, agent::Bridge, client, spawn_notification_forwarder};
use agent_client_protocol::{AgentSideConnection, SessionNotification};
use std::rc::Rc;
use tracing::{error, info};
use trogon_std::time::SystemClock;

#[cfg(not(coverage))]
use {
    acp_nats::nats, acp_telemetry::ServiceName, trogon_std::env::SystemEnv,
    trogon_std::fs::SystemFs,
};

#[cfg(not(coverage))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config = config::base_config(&trogon_std::CliArgs::<config::Args>::new(), &SystemEnv)?;
    acp_telemetry::init_logger(
        ServiceName::AcpNatsStdio,
        config.acp_prefix(),
        &SystemEnv,
        &SystemFs,
    );
    let config = acp_nats::apply_timeout_overrides(config, &SystemEnv);

    info!("ACP bridge starting");

    let nats_connect_timeout = acp_nats::nats_connect_timeout(&SystemEnv);
    let nats_client = nats::connect(config.nats(), nats_connect_timeout).await?;

    let js_context = async_nats::jetstream::new(nats_client.clone());
    let js_client = trogon_nats::jetstream::NatsJetStreamClient::new(js_context);

    let stdin = async_compat::Compat::new(tokio::io::stdin());
    let stdout = async_compat::Compat::new(tokio::io::stdout());

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(run_bridge(
            nats_client,
            js_client,
            &config,
            stdout,
            stdin,
            acp_telemetry::signal::shutdown_signal(),
        ))
        .await;

    if let Err(ref e) = result {
        error!(error = %e, "ACP bridge stopped with error");
    } else {
        info!("ACP bridge stopped");
    }

    acp_telemetry::shutdown_otel();

    result
}

#[cfg(coverage)]
#[cfg_attr(coverage, coverage(off))]
fn main() {}

async fn run_bridge<N, J, W, R>(
    nats_client: N,
    js_client: J,
    config: &acp_nats::Config,
    stdout: W,
    stdin: R,
    shutdown_signal: impl std::future::Future<Output = ()>,
) -> Result<(), Box<dyn std::error::Error>>
where
    N: acp_nats::RequestClient
        + acp_nats::PublishClient
        + acp_nats::FlushClient
        + acp_nats::SubscribeClient
        + 'static,
    J: acp_nats::JetStreamPublisher + acp_nats::JetStreamGetStream + 'static,
    trogon_nats::jetstream::JsMessageOf<J>: trogon_nats::jetstream::JsRequestMessage,
    W: futures::AsyncWrite + Unpin + 'static,
    R: futures::AsyncRead + Unpin + 'static,
{
    let meter = acp_telemetry::meter("acp-io-bridge-nats");
    let (notification_tx, notification_rx) = tokio::sync::mpsc::channel::<SessionNotification>(64);
    let bridge = Rc::new(Bridge::new(
        nats_client.clone(),
        js_client,
        SystemClock,
        &meter,
        config.clone(),
        notification_tx,
    ));

    let (connection, io_task) = AgentSideConnection::new(bridge.clone(), stdout, stdin, |fut| {
        tokio::task::spawn_local(fut);
    });

    let connection = Rc::new(connection);

    spawn_notification_forwarder(connection.clone(), notification_rx);

    let client_connection = connection.clone();
    let bridge_for_client = bridge.clone();
    let mut client_task = tokio::task::spawn_local(client::run(
        nats_client,
        client_connection,
        bridge_for_client,
        StdJsonSerialize,
    ));
    info!("ACP bridge running on stdio with NATS client proxy");

    let shutdown_result = tokio::select! {
        result = &mut client_task => {
            match result {
                Ok(()) => {
                    info!("ACP bridge client task completed");
                    Ok(())
                }
                Err(e) => {
                    error!(error = %e, "Client task ended with error");
                    Err(Box::new(e) as Box<dyn std::error::Error>)
                }
            }
        }
        result = io_task => {
            match result {
                Err(e) => {
                    error!(error = %e, "IO task error");
                    Err(Box::new(e) as Box<dyn std::error::Error>)
                }
                Ok(()) => {
                    info!("ACP bridge shutting down (IO closed)");
                    Ok(())
                }
            }
        }
        _ = shutdown_signal => {
            info!("ACP bridge shutting down (signal received)");
            Ok(())
        }
    };

    if !client_task.is_finished() {
        client_task.abort();
        let _ = client_task.await;
    }

    shutdown_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::{InitializeResponse, ProtocolVersion};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::RwLock;
    use trogon_nats::AdvancedMockNatsClient;

    #[derive(Clone)]
    struct MockJs {
        publisher: trogon_nats::jetstream::MockJetStreamPublisher,
        consumer_factory: trogon_nats::jetstream::MockJetStreamConsumerFactory,
    }

    impl MockJs {
        fn new() -> Self {
            Self {
                publisher: trogon_nats::jetstream::MockJetStreamPublisher::new(),
                consumer_factory: trogon_nats::jetstream::MockJetStreamConsumerFactory::new(),
            }
        }
    }

    impl trogon_nats::jetstream::JetStreamPublisher for MockJs {
        type PublishError = trogon_nats::mocks::MockError;
        type AckFuture = std::future::Ready<
            Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>,
        >;

        async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
            &self,
            subject: S,
            headers: async_nats::HeaderMap,
            payload: bytes::Bytes,
        ) -> Result<Self::AckFuture, Self::PublishError> {
            self.publisher
                .publish_with_headers(subject, headers, payload)
                .await
        }
    }

    impl trogon_nats::jetstream::JetStreamGetStream for MockJs {
        type Error = trogon_nats::mocks::MockError;
        type Stream = trogon_nats::jetstream::MockJetStreamStream;

        async fn get_stream<T: AsRef<str> + Send>(
            &self,
            stream_name: T,
        ) -> Result<trogon_nats::jetstream::MockJetStreamStream, Self::Error> {
            self.consumer_factory.get_stream(stream_name).await
        }
    }

    #[tokio::test]
    async fn run_bridge_shuts_down_on_signal() {
        let mock = AdvancedMockNatsClient::new();
        let _sub = mock.inject_messages();
        let config = acp_nats::Config::new(
            acp_nats::AcpPrefix::new("acp").unwrap(),
            acp_nats::NatsConfig {
                servers: vec!["localhost:4222".to_string()],
                auth: trogon_nats::NatsAuth::None,
            },
        );

        let (reader, _writer) = tokio::io::duplex(1024);
        let (_reader2, writer2) = tokio::io::duplex(1024);
        let stdin = async_compat::Compat::new(reader);
        let stdout = async_compat::Compat::new(writer2);

        let local = tokio::task::LocalSet::new();
        let result = local
            .run_until(run_bridge(
                mock,
                MockJs::new(),
                &config,
                stdout,
                stdin,
                std::future::ready(()),
            ))
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn run_bridge_exits_on_io_close() {
        let mock = AdvancedMockNatsClient::new();
        let _sub = mock.inject_messages();
        let config = acp_nats::Config::new(
            acp_nats::AcpPrefix::new("acp").unwrap(),
            acp_nats::NatsConfig {
                servers: vec!["localhost:4222".to_string()],
                auth: trogon_nats::NatsAuth::None,
            },
        );

        let (reader, writer) = tokio::io::duplex(1024);
        let (_reader2, writer2) = tokio::io::duplex(1024);
        drop(writer);
        let stdin = async_compat::Compat::new(reader);
        let stdout = async_compat::Compat::new(writer2);

        let local = tokio::task::LocalSet::new();
        let result = local
            .run_until(run_bridge(
                mock,
                MockJs::new(),
                &config,
                stdout,
                stdin,
                std::future::pending(),
            ))
            .await;

        assert!(result.is_ok());
    }

    /// E2E: real NATS container + RpcServer + stdio bridge → initialize → response.
    #[cfg_attr(coverage, coverage(off))]
    #[tokio::test]
    async fn e2e_initialize_with_real_nats_returns_protocol_version() {
        use testcontainers_modules::nats::Nats;
        use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
        use acp_nats_agent::AgentSideNatsConnection;
        use trogon_acp_runner::{SessionStore, TrogonAgent};
        use trogon_agent_core::agent_loop::AgentLoop;
        use trogon_agent_core::tools::ToolContext;

        // Start NATS with JetStream.
        let container = Nats::default()
            .with_cmd(["--jetstream"])
            .start()
            .await
            .expect("Docker must be running for this test");
        let port = container.get_host_port_ipv4(4222).await.unwrap();
        let nats_url = format!("127.0.0.1:{port}");

        // Connect clients.
        let nats_for_server = async_nats::connect(&nats_url).await.unwrap();
        let nats_for_bridge = async_nats::connect(&nats_url).await.unwrap();
        let js = async_nats::jetstream::new(nats_for_server.clone());

        // Start TrogonAgent.
        let store = SessionStore::open(&js).await.unwrap();
        let gateway_config = Arc::new(RwLock::new(None));
        let store_clone = store.clone();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let local = tokio::task::LocalSet::new();
            let http = reqwest::Client::new();
            let agent_loop = AgentLoop {
                http_client: http.clone(),
                proxy_url: String::new(),
                anthropic_token: String::new(),
                anthropic_base_url: None,
                anthropic_extra_headers: vec![],
                model: "claude-opus-4-6".to_string(),
                max_iterations: 10,
                thinking_budget: None,
                tool_context: Arc::new(ToolContext { http_client: http, proxy_url: String::new() }),
                memory_owner: None,
                memory_repo: None,
                memory_path: None,
                mcp_tool_defs: vec![],
                mcp_dispatch: vec![],
                permission_checker: None,
            };
            let ta = TrogonAgent::new(
                nats_for_server.clone(),
                store_clone,
                agent_loop,
                "acp",
                "claude-opus-4-6",
                None,
                gateway_config,
            );
            let prefix = acp_nats::AcpPrefix::new("acp").unwrap();
            let (_, ta_io_task) = AgentSideNatsConnection::new(ta, nats_for_server, prefix, |fut| {
                tokio::task::spawn_local(fut);
            });
            rt.block_on(local.run_until(async move { ta_io_task.await.ok(); }));
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Build bridge config.
        let config = acp_nats::Config::new(
            acp_nats::AcpPrefix::new("acp").unwrap(),
            acp_nats::NatsConfig {
                servers: vec![nats_url],
                auth: trogon_nats::NatsAuth::None,
            },
        )
        .with_operation_timeout(Duration::from_secs(5));

        // Create stdio pipes.
        let (stdin_r, mut stdin_w) = tokio::io::duplex(4096);
        let (stdout_r, stdout_w) = tokio::io::duplex(4096);

        // Run bridge in background thread with its own LocalSet.
        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let local = tokio::task::LocalSet::new();
            let stdin = async_compat::Compat::new(stdin_r);
            let stdout = async_compat::Compat::new(stdout_w);
            rt.block_on(local.run_until(run_bridge(
                nats_for_bridge,
                &config,
                stdout,
                stdin,
                std::future::pending::<()>(),
            )))
            .map_err(|e| {
                Box::new(std::io::Error::other(e.to_string()))
                    as Box<dyn std::error::Error + Send + Sync>
            })
        });

        // Send initialize request.
        stdin_w
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":0}}\n",
            )
            .await
            .unwrap();

        // Read response.
        let mut reader = BufReader::new(stdout_r);
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(10), reader.read_line(&mut line))
            .await
            .expect("timed out waiting for initialize response")
            .unwrap();

        drop(stdin_w);
        tokio::task::spawn_blocking(move || handle.join().unwrap().unwrap())
            .await
            .unwrap();

        let response: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(response["id"], serde_json::json!(1));
        assert!(
            response["result"]["protocolVersion"].is_number(),
            "must have protocolVersion: {line}"
        );
    }
}
