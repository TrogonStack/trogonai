//! Integration tests that exercise `CodexAgent` end-to-end using the mock
//! `codex app-server` binary built alongside this crate.
//!
//! All tests set `CODEX_BIN` to the path of that binary so that
//! `CodexProcess::spawn()` forks the mock instead of the real CLI.
//!
//! The tests share a process-wide mutex (`BIN_ENV_LOCK`) to serialise env-var
//! access; `cargo test` runs tests in parallel threads, and `CODEX_BIN` is
//! process-global.
//!
//! A `LocalSet` wraps every async body because `CodexProcess` uses
//! `tokio::task::spawn_local` internally.

use std::sync::{Mutex, OnceLock};

use acp_nats::acp_prefix::AcpPrefix;
use agent_client_protocol::{
    Agent, CancelNotification, CloseSessionRequest, ContentBlock, ForkSessionRequest,
    ListSessionsRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
    ResumeSessionRequest, SetSessionModelRequest, StopReason, TextContent,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::task::LocalSet;
use trogon_codex_runner::CodexAgent;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Path to the compiled mock_codex_server binary.
const MOCK_BIN: &str = env!("CARGO_BIN_EXE_mock_codex_server");

/// Serialise `CODEX_BIN` env-var writes across threads.
static BIN_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn bin_env_lock() -> &'static Mutex<()> {
    BIN_ENV_LOCK.get_or_init(|| Mutex::new(()))
}

/// Minimal fake NATS server — handles the INFO/CONNECT/PING handshake only.
/// `session_notification` in `CodexAgent::prompt()` uses fire-and-forget
/// publish, so the server does not need to respond to PUB commands.
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

/// Build a `CodexAgent` whose `CodexProcess` will spawn the mock server.
///
/// The caller must hold `bin_env_lock()` for the duration of the test so that
/// concurrent tests don't clobber each other's `CODEX_BIN` value.
async fn make_agent() -> CodexAgent {
    unsafe { std::env::set_var("CODEX_BIN", MOCK_BIN) };
    CodexAgent::new(fake_nats().await, AcpPrefix::new("test").unwrap(), "o4-mini")
}

// ── new_session ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn new_session_creates_session_and_returns_state() {
    let _guard = bin_env_lock().lock().unwrap();
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let resp = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();

            // Session ID must be non-empty.
            assert!(!resp.session_id.to_string().is_empty());
            // Modes and models must be populated.
            assert!(resp.modes.is_some(), "modes should be present");
            let models = resp.models.expect("models should be present");
            assert_eq!(models.current_model_id.to_string(), "o4-mini");

            // The session must now appear in list_sessions.
            let list = agent.list_sessions(agent_client_protocol::ListSessionsRequest::new()).await.unwrap();
            assert_eq!(list.sessions.len(), 1);
        })
        .await;
}

// ── resume_session ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn resume_session_succeeds_for_existing_session() {
    let _guard = bin_env_lock().lock().unwrap();
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let new = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            let session_id = new.session_id.to_string();

            agent
                .resume_session(ResumeSessionRequest::new(session_id, "/tmp"))
                .await
                .unwrap();
        })
        .await;
}

// ── fork_session ──────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn fork_session_creates_independent_session_inheriting_model() {
    let _guard = bin_env_lock().lock().unwrap();
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            // Create source session and set a non-default model.
            let src = agent.new_session(NewSessionRequest::new("/src")).await.unwrap();
            let src_id = src.session_id.to_string();
            agent
                .set_session_model(SetSessionModelRequest::new(src_id.clone(), "o3"))
                .await
                .unwrap();

            // Fork it.
            let fork = agent
                .fork_session(ForkSessionRequest::new(src_id, "/fork"))
                .await
                .unwrap();
            let fork_id = fork.session_id.to_string();

            // Forked session must be different from the source.
            assert_ne!(fork_id, src.session_id.to_string());

            // The model override should be inherited.
            let models = fork.models.expect("fork should carry model state");
            assert_eq!(models.current_model_id.to_string(), "o3");

            // Both sessions must be visible in list_sessions.
            let list = agent.list_sessions(agent_client_protocol::ListSessionsRequest::new()).await.unwrap();
            assert_eq!(list.sessions.len(), 2);
        })
        .await;
}

// ── prompt ────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn prompt_returns_end_turn_after_mock_completes() {
    let _guard = bin_env_lock().lock().unwrap();
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            let session_id = sess.session_id.to_string();

            let content = vec![ContentBlock::Text(TextContent::new("ping"))];
            let resp = agent
                .prompt(PromptRequest::new(session_id, content))
                .await
                .unwrap();

            assert_eq!(resp.stop_reason, StopReason::EndTurn);
        })
        .await;
}

// ── multiple turns (process reuse) ───────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn multiple_prompts_reuse_the_same_process() {
    let _guard = bin_env_lock().lock().unwrap();
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            let session_id = sess.session_id.to_string();

            for i in 0..3 {
                let content = vec![ContentBlock::Text(TextContent::new(format!("turn {i}")))];
                let resp = agent
                    .prompt(PromptRequest::new(session_id.clone(), content))
                    .await
                    .unwrap();
                assert_eq!(resp.stop_reason, StopReason::EndTurn, "turn {i} should end cleanly");
            }
        })
        .await;
}

// ── close_session removes session from list ───────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn close_session_removes_it_from_list() {
    let _guard = bin_env_lock().lock().unwrap();
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            let session_id = sess.session_id.to_string();

            agent
                .close_session(agent_client_protocol::CloseSessionRequest::new(session_id))
                .await
                .unwrap();

            let list = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
            assert!(list.sessions.is_empty(), "session should have been removed");
        })
        .await;
}

// ── cancel with live process ──────────────────────────────────────────────────

/// The cancel path that calls `turn_interrupt` was untested: it only fires when
/// the session exists AND the process is alive. This test hits that code path.
#[tokio::test(flavor = "current_thread")]
async fn cancel_calls_turn_interrupt_when_process_alive() {
    let _guard = bin_env_lock().lock().unwrap();
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            // Process is alive (was just spawned for new_session).
            // cancel should call turn_interrupt on the mock and return Ok.
            agent
                .cancel(CancelNotification::new(sess.session_id.to_string()))
                .await
                .unwrap();
        })
        .await;
}

// ── prompt with CodexEvent::Error ────────────────────────────────────────────

/// When the codex process emits an `error` notification during a turn,
/// `prompt()` must break out of the event loop and return EndTurn.
#[tokio::test(flavor = "current_thread")]
async fn prompt_returns_end_turn_on_error_event() {
    let _guard = bin_env_lock().lock().unwrap();
    // Must be set before the process is spawned (env is inherited at fork).
    unsafe { std::env::set_var("MOCK_TURN_SENDS_ERROR", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            let resp = agent
                .prompt(PromptRequest::new(
                    sess.session_id.to_string(),
                    vec![ContentBlock::Text(TextContent::new("hello"))],
                ))
                .await
                .unwrap();
            assert_eq!(resp.stop_reason, StopReason::EndTurn);
        })
        .await;
    unsafe { std::env::remove_var("MOCK_TURN_SENDS_ERROR") };
}

// ── process death drains turn channel ────────────────────────────────────────

/// When the subprocess exits mid-turn, `read_loop` must drain the broadcast
/// channel with a `CodexEvent::Error` so that `prompt()` unblocks instead of
/// hanging until the timeout fires.
#[tokio::test(flavor = "current_thread")]
async fn prompt_unblocks_when_process_exits_mid_turn() {
    let _guard = bin_env_lock().lock().unwrap();
    unsafe { std::env::set_var("MOCK_EXIT_AFTER_TURN_ACK", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();

            // The mock exits immediately after acking turn/start.
            // read_loop must detect EOF and send CodexEvent::Error to the channel.
            let resp = agent
                .prompt(PromptRequest::new(
                    sess.session_id.to_string(),
                    vec![ContentBlock::Text(TextContent::new("test"))],
                ))
                .await
                .unwrap();

            // Should complete (not hang) and return EndTurn.
            assert_eq!(resp.stop_reason, StopReason::EndTurn);
        })
        .await;
    unsafe { std::env::remove_var("MOCK_EXIT_AFTER_TURN_ACK") };
}

// ── process respawn clears stale sessions ────────────────────────────────────

/// When `CodexProcess` is found dead on the next call to `process()`, the
/// agent must clear all in-memory sessions (they're gone from the subprocess)
/// and spawn a fresh process.
#[tokio::test(flavor = "current_thread")]
async fn process_respawns_and_clears_sessions_after_death() {
    let _guard = bin_env_lock().lock().unwrap();
    unsafe { std::env::set_var("MOCK_EXIT_AFTER_TURN_ACK", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            // Step 1: create session1, verify it exists.
            let sess1 = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            let sess1_id = sess1.session_id.to_string();
            assert_eq!(agent.list_sessions(ListSessionsRequest::new()).await.unwrap().sessions.len(), 1);

            // Step 2: run a prompt — process will die after acking turn/start.
            agent
                .prompt(PromptRequest::new(
                    sess1_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("die"))],
                ))
                .await
                .unwrap();

            // Step 3: spawn a new session — process() detects dead process,
            // clears all sessions, and respawns.
            let sess2 = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            let sess2_id = sess2.session_id.to_string();

            // The new session must have a different ID.
            assert_ne!(sess1_id, sess2_id);

            // Only the new session must exist — sess1 was cleared on respawn.
            let list = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
            assert_eq!(list.sessions.len(), 1);
            assert_eq!(list.sessions[0].session_id.to_string(), sess2_id);

            // sess1 must no longer be loadable.
            let err = agent
                .load_session(LoadSessionRequest::new(sess1_id, "/tmp"))
                .await
                .unwrap_err();
            assert!(err.message.contains("not found"));
        })
        .await;
    unsafe { std::env::remove_var("MOCK_EXIT_AFTER_TURN_ACK") };
}

// ── resume_session non-fatal failure ─────────────────────────────────────────

/// `thread/resume` is a best-effort hint. When the mock returns an error,
/// `resume_session` must log a warning but still return `Ok(())`.
#[tokio::test(flavor = "current_thread")]
async fn resume_session_succeeds_when_thread_resume_fails() {
    let _guard = bin_env_lock().lock().unwrap();
    unsafe { std::env::set_var("MOCK_RESUME_FAILS", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            // Should succeed even though thread/resume returns an error.
            agent
                .resume_session(ResumeSessionRequest::new(
                    sess.session_id.to_string(),
                    "/tmp",
                ))
                .await
                .unwrap();
        })
        .await;
    unsafe { std::env::remove_var("MOCK_RESUME_FAILS") };
}

// ── turn/start failure propagates as prompt error ─────────────────────────────

/// When the mock rejects `turn/start`, `prompt()` must return an error.
/// Internally this also exercises the sender-cleanup path in `turn_start`.
#[tokio::test(flavor = "current_thread")]
async fn prompt_returns_error_when_turn_start_fails() {
    let _guard = bin_env_lock().lock().unwrap();
    unsafe { std::env::set_var("MOCK_TURN_START_FAILS", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            let err = agent
                .prompt(PromptRequest::new(
                    sess.session_id.to_string(),
                    vec![ContentBlock::Text(TextContent::new("test"))],
                ))
                .await
                .unwrap_err();
            assert!(
                err.message.contains("turn/start") || !err.message.is_empty(),
                "expected a non-empty error, got: {}",
                err.message
            );
        })
        .await;
    unsafe { std::env::remove_var("MOCK_TURN_START_FAILS") };
}

// ── prompt filters non-text content blocks ────────────────────────────────────

/// Non-text content blocks (images, resource links) must be silently dropped.
/// The prompt must still complete successfully with the text that IS present.
#[tokio::test(flavor = "current_thread")]
async fn prompt_filters_non_text_content_blocks() {
    use agent_client_protocol::ResourceLink;

    let _guard = bin_env_lock().lock().unwrap();
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();

            // Mix a text block with a resource link (non-text).
            let content = vec![
                ContentBlock::Text(TextContent::new("hello")),
                ContentBlock::ResourceLink(ResourceLink::new(
                    "some-resource",
                    "file:///tmp/file.txt",
                )),
                ContentBlock::Text(TextContent::new(" world")),
            ];
            let resp = agent
                .prompt(PromptRequest::new(sess.session_id.to_string(), content))
                .await
                .unwrap();

            // Non-text blocks are ignored; the prompt still completes normally.
            assert_eq!(resp.stop_reason, StopReason::EndTurn);
        })
        .await;
}

// ── new_session fails when codex binary is missing ───────────────────────────

/// When `CODEX_BIN` points to a non-existent binary, `new_session` must
/// return an error instead of panicking.
#[tokio::test(flavor = "current_thread")]
async fn new_session_returns_error_when_codex_not_found() {
    let _guard = bin_env_lock().lock().unwrap();
    unsafe { std::env::set_var("CODEX_BIN", "/nonexistent/path/to/codex") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = CodexAgent::new(
                fake_nats().await,
                AcpPrefix::new("test").unwrap(),
                "o4-mini",
            );
            let err = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap_err();
            assert!(!err.message.is_empty(), "expected a non-empty error message");
        })
        .await;
    // Restore CODEX_BIN so subsequent tests work.
    unsafe { std::env::set_var("CODEX_BIN", MOCK_BIN) };
}

// ── read_loop tolerates malformed JSON lines ──────────────────────────────────

/// If the subprocess emits an unparseable line, `read_loop` must skip it with
/// a warning and continue — the turn must still complete normally.
#[tokio::test(flavor = "current_thread")]
async fn read_loop_skips_malformed_json_and_turn_still_completes() {
    let _guard = bin_env_lock().lock().unwrap();
    unsafe { std::env::set_var("MOCK_MALFORMED_BEFORE_COMPLETE", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            let resp = agent
                .prompt(PromptRequest::new(
                    sess.session_id.to_string(),
                    vec![ContentBlock::Text(TextContent::new("test"))],
                ))
                .await
                .unwrap();
            assert_eq!(resp.stop_reason, StopReason::EndTurn);
        })
        .await;
    unsafe { std::env::remove_var("MOCK_MALFORMED_BEFORE_COMPLETE") };
}

// ── fork_session when thread/fork fails ───────────────────────────────────────

/// When `thread/fork` returns a JSON-RPC error, `fork_session` must propagate
/// it as an ACP error instead of panicking or hanging.
#[tokio::test(flavor = "current_thread")]
async fn fork_session_returns_error_when_thread_fork_fails() {
    let _guard = bin_env_lock().lock().unwrap();
    unsafe { std::env::set_var("MOCK_FORK_FAILS", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let src = agent.new_session(NewSessionRequest::new("/src")).await.unwrap();
            let err = agent
                .fork_session(ForkSessionRequest::new(src.session_id.to_string(), "/fork"))
                .await
                .unwrap_err();
            assert!(!err.message.is_empty(), "expected a non-empty error");
        })
        .await;
    unsafe { std::env::remove_var("MOCK_FORK_FAILS") };
}

// ── new_session when thread/start fails ──────────────────────────────────────

/// When the process spawns successfully but `thread/start` returns a JSON-RPC
/// error, `new_session` must propagate the error.
#[tokio::test(flavor = "current_thread")]
async fn new_session_returns_error_when_thread_start_fails() {
    let _guard = bin_env_lock().lock().unwrap();
    unsafe { std::env::set_var("MOCK_THREAD_START_FAILS", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let err = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap_err();
            assert!(!err.message.is_empty(), "expected a non-empty error");
        })
        .await;
    unsafe { std::env::remove_var("MOCK_THREAD_START_FAILS") };
}

// ── prompt with only non-text content (empty user_input) ─────────────────────

/// When the prompt contains no text blocks at all, `user_input` is empty.
/// The code emits a warning and forwards the empty string to Codex.
/// The turn must still complete normally.
#[tokio::test(flavor = "current_thread")]
async fn prompt_with_only_non_text_blocks_still_completes() {
    use agent_client_protocol::ResourceLink;

    let _guard = bin_env_lock().lock().unwrap();
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            // No text blocks at all — user_input will be "".
            let content = vec![ContentBlock::ResourceLink(ResourceLink::new(
                "some-file",
                "file:///tmp/ctx.txt",
            ))];
            let resp = agent
                .prompt(PromptRequest::new(sess.session_id.to_string(), content))
                .await
                .unwrap();
            assert_eq!(resp.stop_reason, StopReason::EndTurn);
        })
        .await;
}

// ── read_loop ignores response with unknown ID ────────────────────────────────

/// When the subprocess emits a response whose ID has no matching pending
/// request, `read_loop` must silently discard it and let the turn complete.
#[tokio::test(flavor = "current_thread")]
async fn read_loop_ignores_response_with_unknown_id() {
    let _guard = bin_env_lock().lock().unwrap();
    unsafe { std::env::set_var("MOCK_UNKNOWN_ID_BEFORE_COMPLETE", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            let resp = agent
                .prompt(PromptRequest::new(
                    sess.session_id.to_string(),
                    vec![ContentBlock::Text(TextContent::new("test"))],
                ))
                .await
                .unwrap();
            assert_eq!(resp.stop_reason, StopReason::EndTurn);
        })
        .await;
    unsafe { std::env::remove_var("MOCK_UNKNOWN_ID_BEFORE_COMPLETE") };
}

// ── read_loop ignores response with non-integer ID ───────────────────────────

/// When the subprocess emits a response whose ID cannot be parsed as a u64,
/// `read_loop` must warn and continue — the turn must still complete.
#[tokio::test(flavor = "current_thread")]
async fn read_loop_ignores_response_with_noninteger_id() {
    let _guard = bin_env_lock().lock().unwrap();
    unsafe { std::env::set_var("MOCK_NONINT_ID_BEFORE_COMPLETE", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            let resp = agent
                .prompt(PromptRequest::new(
                    sess.session_id.to_string(),
                    vec![ContentBlock::Text(TextContent::new("test"))],
                ))
                .await
                .unwrap();
            assert_eq!(resp.stop_reason, StopReason::EndTurn);
        })
        .await;
    unsafe { std::env::remove_var("MOCK_NONINT_ID_BEFORE_COMPLETE") };
}

// ── prompt timeout ────────────────────────────────────────────────────────────

/// When the subprocess acks `turn/start` but never emits events, the prompt
/// timeout must fire and return `StopReason::EndTurn` instead of hanging.
/// Uses a 1-second timeout to keep the test fast.
#[tokio::test(flavor = "current_thread")]
async fn prompt_times_out_when_no_events_received() {
    let _guard = bin_env_lock().lock().unwrap();
    // Short timeout so the test completes in ~1 s.
    unsafe { std::env::set_var("CODEX_PROMPT_TIMEOUT_SECS", "1") };
    unsafe { std::env::set_var("MOCK_HANG_AFTER_TURN_ACK", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            let resp = agent
                .prompt(PromptRequest::new(
                    sess.session_id.to_string(),
                    vec![ContentBlock::Text(TextContent::new("hang"))],
                ))
                .await
                .unwrap();
            assert_eq!(resp.stop_reason, StopReason::EndTurn);
        })
        .await;
    unsafe { std::env::remove_var("MOCK_HANG_AFTER_TURN_ACK") };
    unsafe { std::env::remove_var("CODEX_PROMPT_TIMEOUT_SECS") };
}

// ── thread/start missing threadId ────────────────────────────────────────────

/// When the subprocess responds to `thread/start` without a `threadId` field,
/// `new_session` must return an error.
#[tokio::test(flavor = "current_thread")]
async fn new_session_returns_error_when_thread_start_response_has_no_thread_id() {
    let _guard = bin_env_lock().lock().unwrap();
    unsafe { std::env::set_var("MOCK_THREAD_START_NO_ID", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let err = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap_err();
            assert!(!err.message.is_empty());
        })
        .await;
    unsafe { std::env::remove_var("MOCK_THREAD_START_NO_ID") };
}

// ── thread/fork missing threadId ─────────────────────────────────────────────

/// When the subprocess responds to `thread/fork` without a `threadId` field,
/// `fork_session` must return an error.
#[tokio::test(flavor = "current_thread")]
async fn fork_session_returns_error_when_fork_response_has_no_thread_id() {
    let _guard = bin_env_lock().lock().unwrap();
    unsafe { std::env::set_var("MOCK_FORK_NO_ID", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let src = agent.new_session(NewSessionRequest::new("/src")).await.unwrap();
            let err = agent
                .fork_session(ForkSessionRequest::new(src.session_id.to_string(), "/fork"))
                .await
                .unwrap_err();
            assert!(!err.message.is_empty());
        })
        .await;
    unsafe { std::env::remove_var("MOCK_FORK_NO_ID") };
}

// ── initialize handshake failure ──────────────────────────────────────────────

/// When `initialize` returns a JSON-RPC error, `CodexProcess::spawn()` must
/// propagate it so that `new_session` returns an error instead of succeeding
/// with an uninitialised process.
#[tokio::test(flavor = "current_thread")]
async fn new_session_returns_error_when_initialize_fails() {
    let _guard = bin_env_lock().lock().unwrap();
    unsafe { std::env::set_var("MOCK_INITIALIZE_FAILS", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let err = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap_err();
            assert!(!err.message.is_empty());
        })
        .await;
    unsafe { std::env::remove_var("MOCK_INITIALIZE_FAILS") };
}

/// When the `initialize` handshake never receives a response, `spawn()` must
/// time out and return an error.  We set `CODEX_SPAWN_TIMEOUT_SECS=1` so the
/// test completes quickly.
#[tokio::test(flavor = "current_thread")]
async fn new_session_returns_error_when_initialize_hangs() {
    let _guard = bin_env_lock().lock().unwrap();
    unsafe { std::env::set_var("MOCK_HANG_ON_INITIALIZE", "1") };
    unsafe { std::env::set_var("CODEX_SPAWN_TIMEOUT_SECS", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let err = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap_err();
            assert!(
                err.message.contains("timed out"),
                "expected timeout error, got: {}",
                err.message
            );
        })
        .await;
    unsafe { std::env::remove_var("MOCK_HANG_ON_INITIALIZE") };
    unsafe { std::env::remove_var("CODEX_SPAWN_TIMEOUT_SECS") };
}

// ── closed session rejects subsequent operations ──────────────────────────────

/// After `close_session`, `load_session`, `resume_session`, and `prompt` must
/// all return errors — the session no longer exists.
#[tokio::test(flavor = "current_thread")]
async fn operations_on_closed_session_return_errors() {
    let _guard = bin_env_lock().lock().unwrap();
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            let sid = sess.session_id.to_string();

            agent
                .close_session(CloseSessionRequest::new(sid.clone()))
                .await
                .unwrap();

            assert!(
                agent
                    .load_session(LoadSessionRequest::new(sid.clone(), "/tmp"))
                    .await
                    .is_err(),
                "load_session should fail after close"
            );
            assert!(
                agent
                    .resume_session(ResumeSessionRequest::new(sid.clone(), "/tmp"))
                    .await
                    .is_err(),
                "resume_session should fail after close"
            );
            assert!(
                agent
                    .prompt(PromptRequest::new(
                        sid.clone(),
                        vec![ContentBlock::Text(TextContent::new("x"))],
                    ))
                    .await
                    .is_err(),
                "prompt should fail after close"
            );
        })
        .await;
}

// ── full session lifecycle ────────────────────────────────────────────────────

/// Exercises the complete lifecycle in one test:
/// create → set model → prompt → fork (inherits model) → prompt fork →
/// close source → verify source is gone, fork still works.
#[tokio::test(flavor = "current_thread")]
async fn full_session_lifecycle() {
    let _guard = bin_env_lock().lock().unwrap();
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            // 1. Create source session and set a non-default model.
            let src = agent.new_session(NewSessionRequest::new("/src")).await.unwrap();
            let src_id = src.session_id.to_string();
            agent
                .set_session_model(SetSessionModelRequest::new(src_id.clone(), "o3"))
                .await
                .unwrap();

            // 2. Prompt on source.
            let r = agent
                .prompt(PromptRequest::new(
                    src_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("step 1"))],
                ))
                .await
                .unwrap();
            assert_eq!(r.stop_reason, StopReason::EndTurn);

            // 3. Fork source — the fork should inherit model "o3".
            let fork = agent
                .fork_session(ForkSessionRequest::new(src_id.clone(), "/fork"))
                .await
                .unwrap();
            let fork_id = fork.session_id.to_string();
            let models = fork.models.expect("fork carries model state");
            assert_eq!(models.current_model_id.to_string(), "o3");

            // 4. Prompt on the fork.
            let r = agent
                .prompt(PromptRequest::new(
                    fork_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("step 2"))],
                ))
                .await
                .unwrap();
            assert_eq!(r.stop_reason, StopReason::EndTurn);

            // 5. Close source — fork must survive.
            agent.close_session(CloseSessionRequest::new(src_id.clone())).await.unwrap();

            let list = agent.list_sessions(ListSessionsRequest::new()).await.unwrap();
            assert_eq!(list.sessions.len(), 1);
            assert_eq!(list.sessions[0].session_id.to_string(), fork_id);

            // 6. Source is gone; fork is still loadable.
            assert!(agent
                .load_session(LoadSessionRequest::new(src_id, "/src"))
                .await
                .is_err());
            assert!(agent
                .load_session(LoadSessionRequest::new(fork_id, "/fork"))
                .await
                .is_ok());
        })
        .await;
}

// ── fork then prompt ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn prompt_on_forked_session_completes() {
    let _guard = bin_env_lock().lock().unwrap();
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let src = agent.new_session(NewSessionRequest::new("/src")).await.unwrap();
            let fork = agent
                .fork_session(ForkSessionRequest::new(src.session_id.to_string(), "/fork"))
                .await
                .unwrap();

            let resp = agent
                .prompt(PromptRequest::new(
                    fork.session_id.to_string(),
                    vec![ContentBlock::Text(TextContent::new("hello from fork"))],
                ))
                .await
                .unwrap();

            assert_eq!(resp.stop_reason, StopReason::EndTurn);
        })
        .await;
}

// ── set_session_model forwards model to subprocess ───────────────────────────

/// After `set_session_model`, the next `prompt` must pass the chosen model ID
/// to `turn/start`. The mock is configured to reject any other model, so the
/// test fails if the model is not forwarded correctly.
#[tokio::test(flavor = "current_thread")]
async fn prompt_forwards_session_model_to_subprocess() {
    let _guard = bin_env_lock().lock().unwrap();
    unsafe { std::env::set_var("MOCK_REQUIRE_MODEL", "o3") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            let session_id = sess.session_id.to_string();

            agent
                .set_session_model(SetSessionModelRequest::new(session_id.clone(), "o3"))
                .await
                .unwrap();

            // Mock rejects turn/start unless model == "o3".
            let resp = agent
                .prompt(PromptRequest::new(
                    session_id,
                    vec![ContentBlock::Text(TextContent::new("test"))],
                ))
                .await
                .unwrap();

            assert_eq!(resp.stop_reason, StopReason::EndTurn);
        })
        .await;
    unsafe { std::env::remove_var("MOCK_REQUIRE_MODEL") };
}

// ── resume then prompt ────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn prompt_after_resume_completes() {
    let _guard = bin_env_lock().lock().unwrap();
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let sess = agent.new_session(NewSessionRequest::new("/tmp")).await.unwrap();
            let session_id = sess.session_id.to_string();

            agent
                .resume_session(ResumeSessionRequest::new(session_id.clone(), "/tmp"))
                .await
                .unwrap();

            let resp = agent
                .prompt(PromptRequest::new(
                    session_id,
                    vec![ContentBlock::Text(TextContent::new("after resume"))],
                ))
                .await
                .unwrap();

            assert_eq!(resp.stop_reason, StopReason::EndTurn);
        })
        .await;
}
// ── concurrent prompts ────────────────────────────────────────────────────────

/// Two sessions prompting simultaneously must each receive their own events
/// and both complete with `EndTurn`.  This exercises the per-thread broadcast
/// channel routing in `CodexProcess` under cooperative concurrency.
#[tokio::test(flavor = "current_thread")]
async fn concurrent_prompts_on_different_sessions_complete_independently() {
    let _guard = bin_env_lock().lock().unwrap();
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let sess1 = agent.new_session(NewSessionRequest::new("/tmp/s1")).await.unwrap();
            let sess2 = agent.new_session(NewSessionRequest::new("/tmp/s2")).await.unwrap();
            let sid1 = sess1.session_id.to_string();
            let sid2 = sess2.session_id.to_string();

            let (r1, r2) = tokio::join!(
                agent.prompt(PromptRequest::new(
                    sid1,
                    vec![ContentBlock::Text(TextContent::new("hello from session 1"))],
                )),
                agent.prompt(PromptRequest::new(
                    sid2,
                    vec![ContentBlock::Text(TextContent::new("hello from session 2"))],
                )),
            );

            assert_eq!(r1.unwrap().stop_reason, StopReason::EndTurn);
            assert_eq!(r2.unwrap().stop_reason, StopReason::EndTurn);
        })
        .await;
}
