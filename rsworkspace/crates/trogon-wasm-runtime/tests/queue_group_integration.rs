//! Integration test for NATS queue-group deduplication in `dispatcher::run`.
//!
//! Verifies that two dispatcher instances sharing the same queue group each
//! receive distinct messages — a single published message is handled by exactly
//! ONE dispatcher.
//!
//! Requires Docker (uses testcontainers to spin up a NATS server).

use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent_client_protocol::{
    CreateTerminalRequest, CreateTerminalResponse, KillTerminalRequest, KillTerminalResponse,
    ReadTextFileRequest, ReadTextFileResponse, ReleaseTerminalRequest, ReleaseTerminalResponse,
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
    SessionNotification, TerminalExitStatus, TerminalId, TerminalOutputRequest,
    TerminalOutputResponse, WaitForTerminalExitRequest, WaitForTerminalExitResponse,
    WriteTextFileRequest, WriteTextFileResponse,
};
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_wasm_runtime::{dispatcher, traits::Runtime};

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let c = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    (c, port)
}

// ── CountingRuntime ───────────────────────────────────────────────────────────

/// Minimal `Runtime` that increments a shared counter on every
/// `handle_create_terminal` call. All other methods return defaults.
struct CountingRuntime {
    counter: Arc<Mutex<usize>>,
}

impl Runtime for CountingRuntime {
    async fn handle_create_terminal(
        &self,
        _session_id: &str,
        _req: CreateTerminalRequest,
    ) -> agent_client_protocol::Result<CreateTerminalResponse> {
        *self.counter.lock().unwrap() += 1;
        Ok(CreateTerminalResponse::new(TerminalId::new("count-tid")))
    }

    async fn handle_terminal_output(
        &self,
        _req: TerminalOutputRequest,
    ) -> agent_client_protocol::Result<TerminalOutputResponse> {
        Ok(TerminalOutputResponse::new(String::new(), false))
    }

    async fn handle_kill_terminal(
        &self,
        _req: KillTerminalRequest,
    ) -> agent_client_protocol::Result<KillTerminalResponse> {
        Ok(KillTerminalResponse::new())
    }

    async fn handle_write_to_terminal(
        &self,
        _terminal_id: &str,
        _data: &[u8],
    ) -> agent_client_protocol::Result<()> {
        Ok(())
    }

    fn handle_close_terminal_stdin(&self, _terminal_id: &str) -> agent_client_protocol::Result<()> {
        Ok(())
    }

    async fn handle_release_terminal(
        &self,
        _req: ReleaseTerminalRequest,
    ) -> agent_client_protocol::Result<ReleaseTerminalResponse> {
        Ok(ReleaseTerminalResponse::new())
    }

    async fn handle_wait_for_terminal_exit(
        &self,
        _req: WaitForTerminalExitRequest,
    ) -> agent_client_protocol::Result<WaitForTerminalExitResponse> {
        Ok(WaitForTerminalExitResponse::new(
            TerminalExitStatus::new().exit_code(Some(0)),
        ))
    }

    async fn handle_write_text_file(
        &self,
        _session_id: &str,
        _req: WriteTextFileRequest,
    ) -> agent_client_protocol::Result<WriteTextFileResponse> {
        Ok(WriteTextFileResponse::new())
    }

    async fn handle_read_text_file(
        &self,
        _session_id: &str,
        _req: ReadTextFileRequest,
    ) -> agent_client_protocol::Result<ReadTextFileResponse> {
        Ok(ReadTextFileResponse::new(String::new()))
    }

    fn handle_request_permission(
        &self,
        _req: RequestPermissionRequest,
    ) -> agent_client_protocol::Result<RequestPermissionResponse> {
        Ok(RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled))
    }

    fn handle_session_notification(&self, _notif: SessionNotification) {}

    fn list_sessions(&self) -> Vec<String> {
        Vec::new()
    }

    fn list_terminals(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    fn cleanup_idle_sessions(&self) {}

    async fn cleanup_all_sessions(&self) {}
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Two dispatcher instances subscribed with the same NATS queue group on the
/// same subject pattern — publishing one `terminal.create` message must cause
/// exactly ONE `handle_create_terminal` call across both runtimes.
#[tokio::test]
async fn queue_group_delivers_message_to_exactly_one_dispatcher() {
    let (_c, port) = start_nats().await;

    let nats1 = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect nats1");
    let nats2 = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect nats2");
    let publisher = async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect publisher");

    let counter = Arc::new(Mutex::new(0usize));

    let (shutdown_tx, shutdown_rx1) = tokio::sync::watch::channel(false);
    let shutdown_rx2 = shutdown_rx1.clone();

    let counter_for_assert = counter.clone();

    let local = tokio::task::LocalSet::new();
    local
        .run_until(async move {
            let runtime1 = Rc::new(CountingRuntime { counter: counter.clone() });
            let runtime2 = Rc::new(CountingRuntime { counter: counter.clone() });

            tokio::task::spawn_local(dispatcher::run(
                nats1,
                "qg-test.session.*.client.>".to_string(),
                runtime1,
                shutdown_rx1,
            ));
            tokio::task::spawn_local(dispatcher::run(
                nats2,
                "qg-test.session.*.client.>".to_string(),
                runtime2,
                shutdown_rx2,
            ));

            // Give both dispatchers time to register their subscriptions.
            tokio::time::sleep(Duration::from_millis(150)).await;

            // Publish one request — the queue group ensures only one dispatcher handles it.
            let req = CreateTerminalRequest::new("sess-qg", "bash");
            let payload = serde_json::to_vec(&req).unwrap();
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                publisher.request(
                    "qg-test.session.sess-qg.client.terminal.create",
                    payload.into(),
                ),
            )
            .await;

            // Give the handling task time to complete.
            tokio::time::sleep(Duration::from_millis(150)).await;

            let _ = shutdown_tx.send(true);
            for _ in 0..10 {
                tokio::task::yield_now().await;
            }
        })
        .await;

    let count = *counter_for_assert.lock().unwrap();
    assert_eq!(
        count, 1,
        "NATS queue group must deliver the message to exactly one dispatcher, got {count}"
    );
}
