//! Regression test for the bash-tool timeout-poison bug (requires Docker).
//!
//! When a bash command exceeds the tool timeout, the persistent terminal is left
//! running the command. If `terminal_id` stays in the session store, the NEXT bash
//! call reuses that terminal and its command queues behind the still-running one —
//! a cascade of timeouts. The fix clears `terminal_id` on timeout so the next call
//! creates a clean terminal.
//!
//! Run with:
//!   cargo test -p trogon-runner-tools --test bash_timeout_poison --features test-helpers

use std::time::Duration;

use agent_client_protocol::schema::v1::{CreateTerminalResponse, TerminalId};
use futures_util::StreamExt as _;
use serde_json::json;
use testcontainers_modules::nats::Nats;
use testcontainers_modules::testcontainers::{ContainerAsync, ImageExt, runners::AsyncRunner};
use trogon_mcp::McpCallTool;
use trogon_runner_tools::session_store::SessionStore;
use trogon_runner_tools::session_store::mock::MemorySessionStore;
use trogon_runner_tools::wasm_bash_tool::WasmRuntimeBashTool;

async fn start_nats() -> (ContainerAsync<Nats>, u16) {
    let c = Nats::default()
        .with_cmd(["--jetstream"])
        .start()
        .await
        .expect("Failed to start NATS container — is Docker running?");
    let port = c.get_host_port_ipv4(4222).await.unwrap();
    (c, port)
}

async fn connect_nats(port: u16) -> async_nats::Client {
    async_nats::connect(format!("127.0.0.1:{port}"))
        .await
        .expect("connect to NATS")
}

/// Spawns a fake wasm-runtime terminal that creates a terminal, ACKs every
/// write_stdin, and answers EVERY output poll WITHOUT an exit marker — so the
/// command appears to run forever and the bash tool hits its timeout.
fn spawn_hanging_terminal(nats: async_nats::Client, base: String, ext_base: String, tid: &'static str) {
    // terminal.create → return a terminal id (so terminal_id gets persisted)
    {
        let nats = nats.clone();
        let base = base.clone();
        tokio::spawn(async move {
            let mut sub = nats.subscribe(format!("{base}.create")).await.unwrap();
            while let Some(msg) = sub.next().await {
                let resp = CreateTerminalResponse::new(TerminalId::new(tid));
                if let Some(reply) = msg.reply {
                    let _ = nats.publish(reply, serde_json::to_vec(&resp).unwrap().into()).await;
                }
            }
        });
    }
    // ext.terminal.write_stdin → ack
    {
        let nats = nats.clone();
        tokio::spawn(async move {
            let mut sub = nats
                .subscribe(format!("{ext_base}.terminal.write_stdin"))
                .await
                .unwrap();
            while let Some(msg) = sub.next().await {
                if let Some(reply) = msg.reply {
                    let _ = nats
                        .publish(reply, serde_json::to_vec(&json!({"ok": true})).unwrap().into())
                        .await;
                }
            }
        });
    }
    // terminal.output → never contains the exit marker → forces a timeout
    {
        let nats = nats.clone();
        tokio::spawn(async move {
            let mut sub = nats.subscribe(format!("{base}.output")).await.unwrap();
            while let Some(msg) = sub.next().await {
                if let Some(reply) = msg.reply {
                    let _ = nats
                        .publish(
                            reply,
                            serde_json::to_vec(&json!({"output": "still running...\n"}))
                                .unwrap()
                                .into(),
                        )
                        .await;
                }
            }
        });
    }
}

#[tokio::test]
async fn bash_timeout_clears_terminal_id_to_avoid_poisoning_later_commands() {
    let (_c, port) = start_nats().await;
    let nats = connect_nats(port).await;

    let prefix = "acp.wasm";
    let session_id = "timeout-poison-sess";
    let base = format!("{prefix}.session.{session_id}.client.terminal");
    let ext_base = format!("{prefix}.session.{session_id}.client.ext");

    spawn_hanging_terminal(nats.clone(), base, ext_base, "tid-hang");
    tokio::time::sleep(Duration::from_millis(50)).await;

    let store = MemorySessionStore::new();
    let tool = WasmRuntimeBashTool::new(
        nats,
        prefix,
        session_id,
        std::path::PathBuf::from("/tmp"),
        Duration::from_millis(400), // short timeout so the test is fast
        store.clone(),
    );

    // The command "runs forever" (the responder never emits the exit marker), so
    // the tool must hit its timeout.
    let result = tool.call_tool("bash", &json!({"command": "sleep 999"})).await;
    assert!(result.is_err(), "command must time out, got: {result:?}");
    assert!(result.unwrap_err().contains("timeout"), "error must be a timeout");

    // The terminal was created (terminal_id was persisted). On timeout it MUST be
    // cleared so the next bash call creates a fresh terminal instead of queueing
    // behind the still-running command.
    let state = store.load(session_id).await.unwrap();
    assert_eq!(
        state.terminal_id, None,
        "terminal_id must be cleared on timeout (a poisoned terminal would block \
         later commands); got {:?}",
        state.terminal_id
    );
}
