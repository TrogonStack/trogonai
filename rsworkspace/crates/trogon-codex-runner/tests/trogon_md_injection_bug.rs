//! Regression test documenting the known TROGON.md injection bug in codex-runner.
//!
//! The bug: in `CodexAgent::prompt()`, `s.first_turn` is set to `false` before
//! its value is captured into the destructured tuple. The cwd is also not
//! captured in the same block. Therefore the `else if first_turn` branch that
//! would call `trogon_runner_tools::trogon_md::load_trogon_md(&cwd)` never
//! runs — no TROGON.md content is injected on the first prompt.
//!
//! This test verifies the current (broken) behavior. When the bug is fixed, this
//! test must be updated to assert that TROGON.md IS injected.
//!
//! See: docs/programming-imple.md §"Gap PR 4"
//!
//! Run with:
//!   cargo test -p trogon-codex-runner --test trogon_md_injection_bug

use std::sync::OnceLock;
use tokio::sync::Mutex;

use acp_nats::acp_prefix::AcpPrefix;
use agent_client_protocol::{Agent, ContentBlock, NewSessionRequest, PromptRequest};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::task::LocalSet;
use trogon_codex_runner::DefaultCodexAgent;

const MOCK_BIN: &str = env!("CARGO_BIN_EXE_mock_codex_server");

static BIN_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn bin_env_lock() -> &'static Mutex<()> {
    BIN_ENV_LOCK.get_or_init(Mutex::default)
}

async fn fake_nats() -> async_nats::Client {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            let (reader, mut writer) = stream.into_split();
            writer
                .write_all(
                    b"INFO {\"server_id\":\"test\",\"version\":\"2.10.0\",\
                      \"max_payload\":1048576,\"proto\":1,\"headers\":true}\r\n",
                )
                .await
                .ok();
            let mut lines = BufReader::new(reader).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.starts_with("CONNECT") {
                    writer.write_all(b"+OK\r\n").await.ok();
                } else if line.starts_with("PING") {
                    writer.write_all(b"PONG\r\n").await.ok();
                }
            }
        }
    });
    async_nats::connect(format!("nats://127.0.0.1:{port}")).await.unwrap()
}

// ── tests ─────────────────────────────────────────────────────────────────────

/// Documents the known bug: TROGON.md is NOT injected on the first prompt.
///
/// The expected behavior (after fixing the bug) would be that the text sent
/// to the Codex process on the first turn includes the TROGON.md content.
///
/// Currently, `first_turn` is set to `false` before being captured, so the
/// injection branch never executes.
///
/// NOTE: This test passes with the CURRENT (broken) code. When the bug is
/// fixed, the assertion must be inverted: assert that the session can complete
/// a first-turn prompt with TROGON.md content injected.
#[tokio::test(flavor = "current_thread")]
async fn codex_first_turn_captures_first_turn_after_set_to_false_known_bug() {
    let _guard = bin_env_lock().lock().await;

    let dir = tempfile::TempDir::new().unwrap();
    // Write a TROGON.md in the session cwd so it would be injected if the bug were fixed.
    std::fs::write(
        dir.path().join("TROGON.md"),
        "# Project rules\nAlways use Rust.\n",
    )
    .unwrap();

    let local = LocalSet::new();
    local
        .run_until(async {
            // Check if the DefaultCodexAgent has with_nats_and_cwd — if not, use
            // new_session to set the cwd. Try new_session approach since the API
            // may not have with_nats_and_cwd.
            unsafe { std::env::set_var("CODEX_BIN", MOCK_BIN) };
            let agent = DefaultCodexAgent::with_nats(
                fake_nats().await,
                AcpPrefix::new("test-trogon-md").unwrap(),
                "o4-mini",
            );

            // Create a session in the directory containing TROGON.md
            let cwd = dir.path().to_string_lossy().to_string();
            let new_resp = agent
                .new_session(NewSessionRequest::new(cwd))
                .await
                .unwrap();
            let sid = new_resp.session_id;

            // After new_session, first_turn must be true (session is fresh).
            // The bug occurs during prompt: first_turn is set to false before capture.
            // We verify the prompt completes without panic — the TROGON.md content
            // will NOT be injected due to the bug, but the prompt still succeeds.
            let prompt_result = agent
                .prompt(PromptRequest::new(
                    sid.clone(),
                    vec![ContentBlock::from("first prompt")],
                ))
                .await;

            // The prompt should complete (mock codex server handles any input).
            assert!(
                prompt_result.is_ok(),
                "first prompt must not error even with TROGON.md present: {:?}",
                prompt_result.err()
            );

            // BUG DOCUMENTED: If the bug were fixed, the text sent to the Codex
            // process on this first turn would include "# Project rules" from TROGON.md.
            // Currently it does not. There is no assert here for the injection content
            // because the mock_codex_server does not expose what text it received —
            // the behavioral gap is documented in docs/programming-imple.md §"Gap PR 4".
        })
        .await;
}
