//! Regression test for TROGON.md injection in codex-runner.
//!
//! History:
//!   1. Original bug: `s.first_turn` was consumed before being captured and
//!      `s.cwd` was never captured, so TROGON.md was never injected.
//!   2. Double-injection bug: two independent injection paths coexisted — an
//!      `else if first_turn` match arm AND the `if prepend_trogon` block — so on
//!      the first turn TROGON.md was prepended TWICE.
//!
//! Current invariant (what this test pins): TROGON.md is injected EXACTLY ONCE on
//! the first turn, through the single `if prepend_trogon` block, and NOT AT ALL on
//! subsequent turns. The count assertion below guards against re-introducing a
//! second injection path — `contains` alone cannot catch duplication.
//!
//! Run with:
//!   cargo test -p trogon-codex-runner --test trogon_md_injection_bug

use std::sync::OnceLock;
use tokio::sync::Mutex;

use acp_nats::acp_prefix::AcpPrefix;
use agent_client_protocol::{Agent, ContentBlock, NewSessionRequest, PromptRequest, TextContent};
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

/// TROGON.md content MUST be prepended to the `userInput` sent to the Codex
/// subprocess on the first turn of a fresh session.
///
/// Verified by setting `MOCK_RECORD_TURN_INPUT_FILE` so the mock binary writes
/// the received `userInput` to a temp file that we inspect after the prompt.
#[tokio::test(flavor = "current_thread")]
async fn codex_first_turn_injects_trogon_md_content_into_subprocess_input() {
    let _guard = bin_env_lock().lock().await;

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("TROGON.md"), "# Project rules\nAlways use Rust.\n").unwrap();

    let record_file = tempfile::NamedTempFile::new().unwrap();
    let record_path = record_file.path().to_str().unwrap().to_string();
    unsafe {
        std::env::set_var("CODEX_BIN", MOCK_BIN);
        std::env::set_var("MOCK_RECORD_TURN_INPUT_FILE", &record_path);
    }

    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = DefaultCodexAgent::with_nats(
                fake_nats().await,
                AcpPrefix::new("test-trogon-md-fix").unwrap(),
                "o4-mini",
            );

            let new_resp = agent.new_session(NewSessionRequest::new(dir.path())).await.unwrap();

            agent
                .prompt(PromptRequest::new(
                    new_resp.session_id,
                    vec![ContentBlock::Text(TextContent::new("first prompt"))],
                ))
                .await
                .expect("first prompt must not error");
        })
        .await;

    unsafe { std::env::remove_var("MOCK_RECORD_TURN_INPUT_FILE") };

    let recorded = std::fs::read_to_string(&record_path).unwrap_or_default();
    assert!(
        recorded.contains("# Project rules"),
        "TROGON.md content must be prepended to userInput on first turn; got: {recorded:?}"
    );
    assert!(
        recorded.contains("Always use Rust"),
        "TROGON.md body must appear in subprocess userInput; got: {recorded:?}"
    );
    assert!(
        recorded.contains("first prompt"),
        "original user message must also be present; got: {recorded:?}"
    );
    // TROGON.md must be injected EXACTLY ONCE. A second injection path (e.g. an
    // `else if first_turn` arm alongside the `if prepend_trogon` block) would
    // duplicate it; `contains` passes either way, so we count occurrences of a
    // marker unique to the TROGON.md body.
    let header_occurrences = recorded.matches("# Project rules").count();
    assert_eq!(
        header_occurrences, 1,
        "TROGON.md must be injected exactly once on the first turn, found {header_occurrences} \
         occurrences (double-injection regression); got: {recorded:?}"
    );
    let body_occurrences = recorded.matches("Always use Rust").count();
    assert_eq!(
        body_occurrences, 1,
        "TROGON.md body must appear exactly once on the first turn, found {body_occurrences} \
         occurrences; got: {recorded:?}"
    );
}

/// TROGON.md content must NOT be injected on the second turn of a session.
///
/// `first_turn` is set to `false` after the first prompt, so subsequent prompts
/// must send only the raw user message to the subprocess. Verified by running
/// two prompts and checking that the recorded userInput from the second turn
/// does not contain TROGON.md content.
#[tokio::test(flavor = "current_thread")]
async fn codex_second_turn_does_not_inject_trogon_md() {
    let _guard = bin_env_lock().lock().await;

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("TROGON.md"), "# Project rules\nAlways use Rust.\n").unwrap();

    let record_file = tempfile::NamedTempFile::new().unwrap();
    let record_path = record_file.path().to_str().unwrap().to_string();
    unsafe {
        std::env::set_var("CODEX_BIN", MOCK_BIN);
        std::env::set_var("MOCK_RECORD_TURN_INPUT_FILE", &record_path);
    }

    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = DefaultCodexAgent::with_nats(
                fake_nats().await,
                AcpPrefix::new("test-trogon-md-second-turn").unwrap(),
                "o4-mini",
            );

            let new_resp = agent.new_session(NewSessionRequest::new(dir.path())).await.unwrap();
            let session_id = new_resp.session_id;

            // First prompt: TROGON.md is injected (first_turn = true).
            agent
                .prompt(PromptRequest::new(
                    session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("first message"))],
                ))
                .await
                .expect("first prompt must not error");

            // Second prompt: first_turn = false, TROGON.md must NOT be injected.
            // The mock overwrites the record file each turn/start, so after this
            // call the file contains only the second turn's userInput.
            agent
                .prompt(PromptRequest::new(
                    session_id,
                    vec![ContentBlock::Text(TextContent::new("second message"))],
                ))
                .await
                .expect("second prompt must not error");
        })
        .await;

    unsafe { std::env::remove_var("MOCK_RECORD_TURN_INPUT_FILE") };

    let recorded = std::fs::read_to_string(&record_path).unwrap_or_default();

    assert!(
        !recorded.contains("# Project rules"),
        "TROGON.md must NOT be injected on the second turn; got: {recorded:?}"
    );
    assert!(
        !recorded.contains("Always use Rust"),
        "TROGON.md body must NOT appear in second turn userInput; got: {recorded:?}"
    );
    assert!(
        recorded.contains("second message"),
        "second turn userInput must contain the user message; got: {recorded:?}"
    );
}
