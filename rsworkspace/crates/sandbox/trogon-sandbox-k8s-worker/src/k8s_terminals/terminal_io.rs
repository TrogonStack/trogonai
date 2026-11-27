use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use async_nats::Client;
use bytes::Bytes;
use tokio::io::{AsyncRead, AsyncReadExt};
use tracing::warn;
use trogon_sandbox_contract::{TerminalId, TerminalOutput};
use trogon_sandbox_protocol::subjects::{terminal_output, worker_lease_activity};
use trogon_sandbox_protocol::{MAX_TERMINAL_OUTPUT_CHUNK_BYTES, terminal_data_frame};

use crate::constants::EXIT_CODE_WAIT;
use crate::terminal_state::TerminalState;
use crate::{K8sTerminals, K8sWorkerError};

pub(crate) async fn publish(client: &Client, id: &TerminalId, frame: &TerminalOutput) {
    match serde_json::to_vec(frame) {
        Ok(payload) => {
            if let Err(error) = client.publish(terminal_output(id), Bytes::from(payload)).await {
                warn!(%error, "terminal output publish failed");
            }
        }
        Err(error) => warn!(%error, "terminal output frame did not serialize"),
    }
}

pub(crate) async fn read_output(
    hub: Arc<K8sTerminals>,
    id: TerminalId,
    terminal: Arc<TerminalState>,
    mut stdout: Pin<Box<dyn AsyncRead + Send>>,
    completion: Pin<Box<dyn Future<Output = Result<i32, K8sWorkerError>> + Send>>,
) {
    let mut buf = vec![0u8; MAX_TERMINAL_OUTPUT_CHUNK_BYTES];
    loop {
        let idle_left = hub.idle_timeout().saturating_sub(terminal.idle_for());
        let read = tokio::select! {
            _ = terminal.stop_reader.cancelled() => return,
            _ = tokio::time::sleep(idle_left) => {
                if terminal.idle_for() >= hub.idle_timeout() {
                    hub.finish(&id, None, true).await;
                    return;
                }
                continue;
            }
            read = stdout.read(&mut buf) => read,
        };
        let n = match read {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        terminal.touch();
        {
            let mut seq = terminal.seq.lock().await;
            match terminal_data_frame(*seq, &buf[..n]) {
                Ok(frame) => {
                    terminal
                        .replay
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .push(*seq, &buf[..n]);
                    publish(hub.client(), &id, &frame).await;
                    *seq += 1;
                }
                Err(error) => warn!(%error, "terminal output frame did not encode"),
            }
        }
        if terminal.activity.due(Instant::now()) {
            let subject = worker_lease_activity(&terminal.lease_id);
            if let Err(error) = hub.client().publish(subject, Bytes::new()).await {
                warn!(%error, "terminal activity signal publish failed");
            }
        }
    }
    // The stream ended on its own: the command exited.
    let exit_code = match tokio::time::timeout(EXIT_CODE_WAIT, completion).await {
        Ok(Ok(code)) => Some(code),
        _ => None,
    };
    hub.finish(&id, exit_code, false).await;
}
