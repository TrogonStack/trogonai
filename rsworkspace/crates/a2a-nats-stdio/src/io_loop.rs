use a2a_nats::client::Client;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use tracing::{debug, error, warn};
use trogon_nats::jetstream::{JetStreamCreateConsumer, JetStreamGetStream, JsAck, JsMessageOf, JsMessageRef};
use trogon_nats::RequestClient;

use crate::dispatch::dispatch_request;
use crate::wire::{InboundRequest, OutboundError, OutboundFrame, RpcId};

const CHANNEL_CAP: usize = 128;

pub async fn run_io_loop<N, J, R, W>(
    client: Client<N, J>,
    stdin: R,
    mut stdout: W,
    shutdown: impl std::future::Future<Output = ()>,
) where
    N: RequestClient + Clone + Send + Sync + 'static,
    J: JetStreamGetStream + Clone + Send + Sync + 'static,
    JsMessageOf<J>: JsMessageRef + JsAck<Error: std::fmt::Display + Send + 'static> + Send + 'static,
    <J as JetStreamGetStream>::Stream: Send + 'static,
    <<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::Messages: Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::MessagesError: std::fmt::Display + Send + 'static,
    <<<J as JetStreamGetStream>::Stream as JetStreamCreateConsumer>::Consumer as trogon_nats::jetstream::JetStreamConsumer>::StreamError: std::fmt::Display + Send + 'static,
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (frame_tx, mut frame_rx) = mpsc::channel::<OutboundFrame>(CHANNEL_CAP);

    let writer_task = tokio::spawn(async move {
        while let Some(frame) = frame_rx.recv().await {
            match serde_json::to_string(&frame) {
                Ok(json) => {
                    if let Err(e) = stdout.write_all(json.as_bytes()).await {
                        error!(error = %e, "stdout write failed");
                        return;
                    }
                    if let Err(e) = stdout.write_all(b"\n").await {
                        error!(error = %e, "stdout write failed");
                        return;
                    }
                }
                Err(e) => {
                    error!(error = %e, "frame serialization failed");
                }
            }
        }
    });

    let client = std::sync::Arc::new(client);
    let mut lines = BufReader::new(stdin).lines();

    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            _ = &mut shutdown => {
                debug!("io_loop received shutdown signal");
                break;
            }
            line = lines.next_line() => {
                match line {
                    Err(e) => {
                        error!(error = %e, "stdin read error");
                        break;
                    }
                    Ok(None) => {
                        debug!("stdin closed");
                        break;
                    }
                    Ok(Some(raw)) => {
                        let raw = raw.trim().to_owned();
                        if raw.is_empty() {
                            continue;
                        }
                        debug!(raw = %raw, "received line");

                        let req: InboundRequest = match serde_json::from_str(&raw) {
                            Ok(r) => r,
                            Err(e) => {
                                warn!(error = %e, "failed to parse JSON-RPC request");
                                let frame = OutboundFrame::Error(OutboundError::new(
                                    RpcId::Null,
                                    -32700,
                                    format!("parse error: {e}"),
                                ));
                                let _ = frame_tx.send(frame).await;
                                continue;
                            }
                        };

                        let id = req.id;
                        let method = req.method;
                        let params = req.params;
                        let client = client.clone();
                        let tx = frame_tx.clone();
                        tokio::spawn(async move {
                            dispatch_request(&client, id, &method, params, &tx).await;
                        });
                    }
                }
            }
        }
    }

    drop(frame_tx);
    let _ = writer_task.await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use a2a_nats::client::Client;
    use a2a_nats::{A2aAgentId, A2aPrefix, Config, NatsConfig};
    use bytes::Bytes;
    use tokio::io::AsyncReadExt;
    use tokio::io::AsyncWriteExt;
    use trogon_nats::AdvancedMockNatsClient;
    use trogon_nats::jetstream::mocks::MockJetStreamConsumerFactory;

    fn test_config() -> Config {
        Config::new(
            A2aPrefix::new("a2a".to_string()).unwrap(),
            NatsConfig { servers: vec!["localhost:4222".to_string()], auth: trogon_nats::NatsAuth::None },
        )
    }

    fn make_client(
        nats: AdvancedMockNatsClient,
        js: MockJetStreamConsumerFactory,
    ) -> Client<AdvancedMockNatsClient, MockJetStreamConsumerFactory> {
        let agent_id = A2aAgentId::new("bot").unwrap();
        Client::new(test_config(), agent_id, nats, js)
    }

    fn task_response(task_id: &str) -> Bytes {
        let task = a2a_types::Task {
            id: task_id.to_string(),
            status: Some(a2a_types::TaskStatus {
                state: a2a_types::TaskState::Completed.into(),
                message: None,
                timestamp: None,
            }),
            ..Default::default()
        };
        serde_json::to_vec(&serde_json::json!({
            "jsonrpc": "2.0", "id": "x", "result": task
        }))
        .unwrap()
        .into()
    }

    #[tokio::test]
    async fn io_loop_exits_on_eof() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());

        let (stdin_reader, _stdin_writer) = tokio::io::duplex(1024);
        let (_stdout_reader, stdout_writer) = tokio::io::duplex(1024);
        drop(_stdin_writer);

        run_io_loop(client, stdin_reader, stdout_writer, std::future::pending::<()>()).await;
    }

    #[tokio::test]
    async fn io_loop_exits_on_shutdown_signal() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());

        let (stdin_reader, _stdin_writer) = tokio::io::duplex(1024);
        let (_stdout_reader, stdout_writer) = tokio::io::duplex(1024);

        run_io_loop(client, stdin_reader, stdout_writer, std::future::ready(())).await;
    }

    #[tokio::test]
    async fn io_loop_handles_valid_request() {
        let nats = AdvancedMockNatsClient::new();
        nats.set_response("a2a.agent.bot.tasks.get", task_response("t1"));
        let client = make_client(nats, MockJetStreamConsumerFactory::new());

        let (stdin_reader, mut stdin_writer) = tokio::io::duplex(4096);
        let (mut stdout_reader, stdout_writer) = tokio::io::duplex(4096);

        let request = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"tasks/get\",\"params\":{\"id\":\"t1\",\"tenant\":\"\",\"historyLength\":null}}\n";
        stdin_writer.write_all(request).await.unwrap();
        drop(stdin_writer);

        run_io_loop(client, stdin_reader, stdout_writer, std::future::pending::<()>()).await;

        let mut buf = vec![0u8; 4096];
        let n = stdout_reader.read(&mut buf).await.unwrap();
        let output = String::from_utf8_lossy(&buf[..n]);
        assert!(output.contains("\"result\""), "expected result in: {output}");
        assert!(output.contains("t1"));
    }

    #[tokio::test]
    async fn io_loop_handles_parse_error() {
        let nats = AdvancedMockNatsClient::new();
        let client = make_client(nats, MockJetStreamConsumerFactory::new());

        let (stdin_reader, mut stdin_writer) = tokio::io::duplex(4096);
        let (mut stdout_reader, stdout_writer) = tokio::io::duplex(4096);

        stdin_writer.write_all(b"not valid json\n").await.unwrap();
        drop(stdin_writer);

        run_io_loop(client, stdin_reader, stdout_writer, std::future::pending::<()>()).await;

        let mut buf = vec![0u8; 4096];
        let n = stdout_reader.read(&mut buf).await.unwrap();
        let output = String::from_utf8_lossy(&buf[..n]);
        assert!(output.contains("\"error\""), "expected error in: {output}");
        assert!(output.contains("-32700"));
    }
}
