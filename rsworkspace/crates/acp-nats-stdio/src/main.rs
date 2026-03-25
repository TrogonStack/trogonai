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

    let stdin = async_compat::Compat::new(tokio::io::stdin());
    let stdout = async_compat::Compat::new(tokio::io::stdout());

    let local = tokio::task::LocalSet::new();
    let result = local
        .run_until(run_bridge(
            nats_client,
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
fn main() {}

async fn run_bridge<N, W, R>(
    nats_client: N,
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
    W: futures::AsyncWrite + Unpin + 'static,
    R: futures::AsyncRead + Unpin + 'static,
{
    let meter = acp_telemetry::meter("acp-io-bridge-nats");
    let (notification_tx, notification_rx) = tokio::sync::mpsc::channel::<SessionNotification>(64);
    let bridge = Rc::new(Bridge::new(
        nats_client.clone(),
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
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use trogon_nats::AdvancedMockNatsClient;

    fn make_config() -> acp_nats::Config {
        acp_nats::Config::new(
            acp_nats::AcpPrefix::new("acp").unwrap(),
            acp_nats::NatsConfig {
                servers: vec!["localhost:4222".to_string()],
                auth: trogon_nats::NatsAuth::None,
            },
        )
    }

    /// Starts the bridge in a background OS thread with its own Tokio runtime and LocalSet.
    /// Returns a handle to the thread and both ends of the stdio pipes.
    fn start_bridge_thread(
        mock: AdvancedMockNatsClient,
        config: acp_nats::Config,
    ) -> (
        std::thread::JoinHandle<Result<(), Box<dyn std::error::Error + Send + Sync>>>,
        tokio::io::DuplexStream, // write end (stdin for bridge)
        tokio::io::DuplexStream, // read end (stdout from bridge)
    ) {
        let (stdin_r, stdin_w) = tokio::io::duplex(4096);
        let (stdout_r, stdout_w) = tokio::io::duplex(4096);

        let handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let local = tokio::task::LocalSet::new();
            let stdin = async_compat::Compat::new(stdin_r);
            let stdout = async_compat::Compat::new(stdout_w);
            rt.block_on(local.run_until(run_bridge(
                mock,
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

        (handle, stdin_w, stdout_r)
    }

    #[tokio::test]
    async fn run_bridge_initialize_request_gets_response() {
        let mock = AdvancedMockNatsClient::new();
        let _sub = mock.inject_messages();
        let init_resp = InitializeResponse::new(ProtocolVersion::LATEST);
        mock.set_response(
            "acp.agent.initialize",
            serde_json::to_vec(&init_resp).unwrap().into(),
        );

        let (bridge_handle, mut stdin_w, stdout_r) = start_bridge_thread(mock, make_config());

        stdin_w
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":0}}\n",
            )
            .await
            .unwrap();

        let mut reader = BufReader::new(stdout_r);
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
            .await
            .expect("timed out waiting for initialize response")
            .unwrap();

        drop(stdin_w); // close stdin → bridge exits
        tokio::task::spawn_blocking(move || bridge_handle.join().unwrap().unwrap())
            .await
            .unwrap();

        assert!(!line.trim().is_empty(), "expected non-empty response");
        let response: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(response["id"], serde_json::json!(1));
        assert!(response["result"].is_object(), "expected result object");
    }

    #[tokio::test]
    async fn run_bridge_invalid_json_does_not_crash_server() {
        let mock = AdvancedMockNatsClient::new();
        let _sub = mock.inject_messages();
        let init_resp = InitializeResponse::new(ProtocolVersion::LATEST);
        mock.set_response(
            "acp.agent.initialize",
            serde_json::to_vec(&init_resp).unwrap().into(),
        );

        let (bridge_handle, mut stdin_w, stdout_r) = start_bridge_thread(mock, make_config());

        // Send invalid JSON first
        stdin_w.write_all(b"this is not json\n").await.unwrap();

        // Then send a valid initialize request — bridge must still respond
        stdin_w
            .write_all(
                b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"initialize\",\"params\":{\"protocolVersion\":0}}\n",
            )
            .await
            .unwrap();

        let mut reader = BufReader::new(stdout_r);
        let mut line = String::new();
        tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut line))
            .await
            .expect("timed out — server may have crashed on invalid JSON")
            .unwrap();

        drop(stdin_w);
        tokio::task::spawn_blocking(move || bridge_handle.join().unwrap().unwrap())
            .await
            .unwrap();

        let response: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
        assert_eq!(response["id"], serde_json::json!(2));
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
                &config,
                stdout,
                stdin,
                std::future::pending(),
            ))
            .await;

        assert!(result.is_ok());
    }
}
