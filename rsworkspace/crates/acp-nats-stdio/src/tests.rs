use super::*;
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
    type AckFuture = std::future::Ready<Result<async_nats::jetstream::publish::PublishAck, Self::PublishError>>;

    async fn publish_with_headers<S: async_nats::subject::ToSubject + Send>(
        &self,
        subject: S,
        headers: async_nats::HeaderMap,
        payload: bytes::Bytes,
    ) -> Result<Self::AckFuture, Self::PublishError> {
        self.publisher.publish_with_headers(subject, headers, payload).await
    }
}

impl trogon_nats::jetstream::JetStreamGetStream for MockJs {
    type Error = async_nats::jetstream::context::GetStreamError;
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

/// E2E: real NATS container + TrogonAgent + stdio bridge -> initialize -> response.
#[cfg_attr(coverage, coverage(off))]
#[tokio::test]
async fn e2e_initialize_with_real_nats_returns_protocol_version() {
    use acp_nats_agent::AgentSideNatsConnection;
    use testcontainers_modules::nats::Nats;
    use testcontainers_modules::testcontainers::{ImageExt, runners::AsyncRunner};
    use trogon_acp_runner::{NatsSessionNotifier, NatsSessionStore, TrogonAgent};
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

    // Start TrogonAgent in its own thread with its own Tokio runtime.
    // NatsSessionStore must be opened inside that runtime to avoid cross-runtime issues.
    let gateway_config = Arc::new(RwLock::new(None));
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
            tool_context: Arc::new(ToolContext {
                proxy_url: String::new(),
                cwd: String::new(),
                http_client: http.clone(),
                web_search_api_key: None,
                web_search_endpoint: None,
            }),
            memory_owner: None,
            memory_repo: None,
            memory_path: None,
            mcp_tool_defs: vec![],
            mcp_dispatch: vec![],
            permission_checker: None,
            elicitation_provider: None,
            post_tool_observer: None,
            streaming_client: None,
        };
        let store = rt.block_on(async {
            let js = async_nats::jetstream::new(nats_for_server.clone());
            NatsSessionStore::open(&js).await.unwrap()
        });
        let ta = TrogonAgent::new(
            NatsSessionNotifier::new(nats_for_server.clone()),
            store,
            agent_loop,
            "acp",
            "claude-opus-4-6",
            None,
            None,
            gateway_config,
        );
        let prefix = acp_nats::AcpPrefix::new("acp").unwrap();
        let (_, ta_io_task) = AgentSideNatsConnection::new(ta, nats_for_server, prefix, |fut| {
            tokio::task::spawn_local(fut);
        });
        rt.block_on(local.run_until(async move {
            ta_io_task.await.ok();
        }));
    });
    tokio::time::sleep(Duration::from_millis(500)).await;

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
    // async_nats::jetstream::new spawns an internal acker task, so it must be
    // called inside the runtime context (within block_on).
    let handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let local = tokio::task::LocalSet::new();
        let stdin = async_compat::Compat::new(stdin_r);
        let stdout = async_compat::Compat::new(stdout_w);
        rt.block_on(local.run_until(async move {
            let js_bridge = async_nats::jetstream::new(nats_for_bridge.clone());
            let js_client_bridge = trogon_nats::jetstream::NatsJetStreamClient::new(js_bridge);
            run_bridge(
                nats_for_bridge,
                js_client_bridge,
                &config,
                stdout,
                stdin,
                std::future::pending::<()>(),
            )
            .await
        }))
        .map_err(|e| Box::new(std::io::Error::other(e.to_string())) as Box<dyn std::error::Error + Send + Sync>)
    });

    // Send initialize request.
    stdin_w
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":0}}\n")
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
