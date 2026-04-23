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

use std::sync::OnceLock;
use tokio::sync::Mutex;

use acp_nats::acp_prefix::AcpPrefix;
use agent_client_protocol::{
    Agent, CancelNotification, CloseSessionRequest, ContentBlock, ExtRequest, ForkSessionRequest,
    ListSessionsRequest, LoadSessionRequest, NewSessionRequest, PromptRequest,
    ResumeSessionRequest, SetSessionConfigOptionRequest, SetSessionModeRequest,
    SetSessionModelRequest, StopReason, TextContent,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::task::LocalSet;
use trogon_codex_runner::DefaultCodexAgent;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Path to the compiled mock_codex_server binary.
const MOCK_BIN: &str = env!("CARGO_BIN_EXE_mock_codex_server");

/// Serialise `CODEX_BIN` env-var writes across threads.
static BIN_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn bin_env_lock() -> &'static Mutex<()> {
    BIN_ENV_LOCK.get_or_init(Mutex::default)
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

    async_nats::connect(format!("nats://127.0.0.1:{port}"))
        .await
        .unwrap()
}

/// Build a `DefaultCodexAgent` whose `CodexProcess` will spawn the mock server.
///
/// The caller must hold `bin_env_lock()` for the duration of the test so that
/// concurrent tests don't clobber each other's `CODEX_BIN` value.
async fn make_agent() -> DefaultCodexAgent {
    unsafe { std::env::set_var("CODEX_BIN", MOCK_BIN) };
    DefaultCodexAgent::with_nats(
        fake_nats().await,
        AcpPrefix::new("test").unwrap(),
        "o4-mini",
    )
}

// ── new_session ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn new_session_creates_session_and_returns_state() {
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let resp = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();

            // Session ID must be non-empty.
            assert!(!resp.session_id.to_string().is_empty());
            // Modes and models must be populated.
            assert!(resp.modes.is_some(), "modes should be present");
            let models = resp.models.expect("models should be present");
            assert_eq!(models.current_model_id.to_string(), "o4-mini");

            // The session must now appear in list_sessions.
            let list = agent
                .list_sessions(agent_client_protocol::ListSessionsRequest::new())
                .await
                .unwrap();
            assert_eq!(list.sessions.len(), 1);
        })
        .await;
}

// ── resume_session ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn resume_session_succeeds_for_existing_session() {
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let new = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            // Create source session and set a non-default model.
            let src = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
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
            let list = agent
                .list_sessions(agent_client_protocol::ListSessionsRequest::new())
                .await
                .unwrap();
            assert_eq!(list.sessions.len(), 2);
        })
        .await;
}

// ── prompt ────────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn prompt_returns_end_turn_after_mock_completes() {
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let session_id = sess.session_id.to_string();

            for i in 0..3 {
                let content = vec![ContentBlock::Text(TextContent::new(format!("turn {i}")))];
                let resp = agent
                    .prompt(PromptRequest::new(session_id.clone(), content))
                    .await
                    .unwrap();
                assert_eq!(
                    resp.stop_reason,
                    StopReason::EndTurn,
                    "turn {i} should end cleanly"
                );
            }
        })
        .await;
}

// ── close_session removes session from list ───────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn close_session_removes_it_from_list() {
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let session_id = sess.session_id.to_string();

            agent
                .close_session(agent_client_protocol::CloseSessionRequest::new(session_id))
                .await
                .unwrap();

            let list = agent
                .list_sessions(ListSessionsRequest::new())
                .await
                .unwrap();
            assert!(list.sessions.is_empty(), "session should have been removed");
        })
        .await;
}

// ── cancel with live process ──────────────────────────────────────────────────

/// The cancel path that calls `turn_interrupt` was untested: it only fires when
/// the session exists AND the process is alive. This test hits that code path.
#[tokio::test(flavor = "current_thread")]
async fn cancel_calls_turn_interrupt_when_process_alive() {
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
    // Must be set before the process is spawned (env is inherited at fork).
    unsafe { std::env::set_var("MOCK_TURN_SENDS_ERROR", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_EXIT_AFTER_TURN_ACK", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();

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
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_EXIT_AFTER_TURN_ACK", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            // Step 1: create session1, verify it exists.
            let sess1 = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let sess1_id = sess1.session_id.to_string();
            assert_eq!(
                agent
                    .list_sessions(ListSessionsRequest::new())
                    .await
                    .unwrap()
                    .sessions
                    .len(),
                1
            );

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
            let sess2 = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let sess2_id = sess2.session_id.to_string();

            // The new session must have a different ID.
            assert_ne!(sess1_id, sess2_id);

            // Only the new session must exist — sess1 was cleared on respawn.
            let list = agent
                .list_sessions(ListSessionsRequest::new())
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_RESUME_FAILS", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_TURN_START_FAILS", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
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

    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();

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
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("CODEX_BIN", "/nonexistent/path/to/codex") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = DefaultCodexAgent::with_nats(
                fake_nats().await,
                AcpPrefix::new("test").unwrap(),
                "o4-mini",
            );
            let err = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap_err();
            assert!(
                !err.message.is_empty(),
                "expected a non-empty error message"
            );
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
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_MALFORMED_BEFORE_COMPLETE", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_FORK_FAILS", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let src = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
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

    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_UNKNOWN_ID_BEFORE_COMPLETE", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_NONINT_ID_BEFORE_COMPLETE", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
    // Short timeout so the test completes in ~1 s.
    unsafe { std::env::set_var("CODEX_PROMPT_TIMEOUT_SECS", "1") };
    unsafe { std::env::set_var("MOCK_HANG_AFTER_TURN_ACK", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
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
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_FORK_NO_ID", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let src = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
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
    let _guard = bin_env_lock().lock().await;
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
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            // 1. Create source session and set a non-default model.
            let src = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
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
            agent
                .close_session(CloseSessionRequest::new(src_id.clone()))
                .await
                .unwrap();

            let list = agent
                .list_sessions(ListSessionsRequest::new())
                .await
                .unwrap();
            assert_eq!(list.sessions.len(), 1);
            assert_eq!(list.sessions[0].session_id.to_string(), fork_id);

            // 6. Source is gone; fork is still loadable.
            assert!(
                agent
                    .load_session(LoadSessionRequest::new(src_id, "/src"))
                    .await
                    .is_err()
            );
            assert!(
                agent
                    .load_session(LoadSessionRequest::new(fork_id, "/fork"))
                    .await
                    .is_ok()
            );
        })
        .await;
}

// ── fork then prompt ──────────────────────────────────────────────────────────

#[tokio::test(flavor = "current_thread")]
async fn prompt_on_forked_session_completes() {
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let src = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_REQUIRE_MODEL", "o3") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
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
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let sess1 = agent
                .new_session(NewSessionRequest::new("/tmp/s1"))
                .await
                .unwrap();
            let sess2 = agent
                .new_session(NewSessionRequest::new("/tmp/s2"))
                .await
                .unwrap();
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

// ── new_session error does not register the session ───────────────────────────

/// When `thread/start` fails, `new_session` must return an error AND must NOT
/// insert a partial session into the session map.  A subsequent `list_sessions`
/// must return an empty list.
#[tokio::test(flavor = "current_thread")]
async fn new_session_error_leaves_session_list_empty() {
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_THREAD_START_FAILS", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let _ = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap_err();

            let list = agent
                .list_sessions(ListSessionsRequest::new())
                .await
                .unwrap();
            assert!(
                list.sessions.is_empty(),
                "a failed new_session must not register a session (got {:?})",
                list.sessions
            );
        })
        .await;
    unsafe { std::env::remove_var("MOCK_THREAD_START_FAILS") };
}

// ── cancel when turn/interrupt fails is non-fatal ─────────────────────────────

/// `turn/interrupt` failures must be logged but must NOT cause `cancel()` to
/// return an error — the ACP contract is fire-and-forget for cancel.
#[tokio::test(flavor = "current_thread")]
async fn cancel_turn_interrupt_failure_is_non_fatal() {
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_INTERRUPT_FAILS", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();

            // turn/interrupt will return an error from the mock, but cancel()
            // must swallow it and return Ok.
            agent
                .cancel(CancelNotification::new(sess.session_id.to_string()))
                .await
                .unwrap();
        })
        .await;
    unsafe { std::env::remove_var("MOCK_INTERRUPT_FAILS") };
}

// ── prompt receives all events when mock emits many ───────────────────────────

/// When the subprocess emits many text-delta events in one turn, all of them
/// must be received by `prompt()` and the turn must complete with `EndTurn`.
/// Uses `MOCK_SEND_N_TEXT_EVENTS=100` to emit 100 events per turn.
#[tokio::test(flavor = "current_thread")]
async fn prompt_receives_many_text_events_and_completes() {
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_SEND_N_TEXT_EVENTS", "100") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();

            let resp = agent
                .prompt(PromptRequest::new(
                    sess.session_id.to_string(),
                    vec![ContentBlock::Text(TextContent::new("flood"))],
                ))
                .await
                .unwrap();

            assert_eq!(resp.stop_reason, StopReason::EndTurn);
        })
        .await;
    unsafe { std::env::remove_var("MOCK_SEND_N_TEXT_EVENTS") };
}

// ── process death unblocks all active turn channels ──────────────────────────

/// When the subprocess exits while two sessions are active (but not mid-turn),
/// calling `new_session` afterwards must detect the dead process, clear the
/// stale sessions, and spawn a fresh subprocess.
///
/// This covers the `read_loop` EOF path that drains `turn_senders` so that any
/// in-flight `event_rx.recv()` callers unblock rather than hanging.
#[tokio::test(flavor = "current_thread")]
async fn process_death_clears_all_sessions_on_next_call() {
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_EXIT_AFTER_TURN_ACK", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            // Create two sessions.
            let s1 = agent
                .new_session(NewSessionRequest::new("/s1"))
                .await
                .unwrap();
            let s2 = agent
                .new_session(NewSessionRequest::new("/s2"))
                .await
                .unwrap();
            assert_eq!(
                agent
                    .list_sessions(ListSessionsRequest::new())
                    .await
                    .unwrap()
                    .sessions
                    .len(),
                2
            );

            // Kill the process by running a prompt (process exits after acking).
            agent
                .prompt(PromptRequest::new(
                    s1.session_id.to_string(),
                    vec![ContentBlock::Text(TextContent::new("die"))],
                ))
                .await
                .unwrap();

            // Next call detects the dead process, clears sessions, and respawns.
            let s3 = agent
                .new_session(NewSessionRequest::new("/s3"))
                .await
                .unwrap();

            // s1 and s2 must be gone; only s3 remains.
            let list = agent
                .list_sessions(ListSessionsRequest::new())
                .await
                .unwrap();
            assert_eq!(list.sessions.len(), 1);
            assert_eq!(
                list.sessions[0].session_id.to_string(),
                s3.session_id.to_string()
            );

            // s2 must no longer be loadable.
            assert!(
                agent
                    .load_session(LoadSessionRequest::new(s2.session_id.to_string(), "/s2"))
                    .await
                    .is_err(),
                "s2 should be gone after process respawn"
            );
        })
        .await;
    unsafe { std::env::remove_var("MOCK_EXIT_AFTER_TURN_ACK") };
}

// ── Drop kills subprocess ─────────────────────────────────────────────────────

/// `Drop::drop` calls `start_kill()` on the child process. After the process
/// exits, `read_loop` detects EOF and sets `alive = false`. This test verifies
/// that chain by observing the alive flag after the struct is dropped.
#[tokio::test(flavor = "current_thread")]
async fn drop_kills_subprocess() {
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            unsafe { std::env::set_var("CODEX_BIN", MOCK_BIN) };
            let proc = trogon_codex_runner::CodexProcess::spawn().await.unwrap();
            let alive = proc.alive_flag();
            assert!(
                alive.load(std::sync::atomic::Ordering::Relaxed),
                "process should be alive right after spawn"
            );

            // Drop the process — triggers start_kill() → SIGKILL.
            drop(proc);

            // Give the subprocess a moment to die and read_loop to detect EOF.
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;

            assert!(
                !alive.load(std::sync::atomic::Ordering::Relaxed),
                "alive flag should be false after process is dropped"
            );
        })
        .await;
}

// ── stray thread notification ─────────────────────────────────────────────────

/// When the subprocess emits a notification for a `threadId` that has no
/// active turn sender, `read_loop` must silently discard it. The current
/// session's turn must still complete normally.
#[tokio::test(flavor = "current_thread")]
async fn stray_notification_is_silently_discarded() {
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_EMIT_STRAY_THREAD_EVENT", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
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
    unsafe { std::env::remove_var("MOCK_EMIT_STRAY_THREAD_EVENT") };
}

// ── NATS publish failure is non-fatal ────────────────────────────────────────

/// A failing NATS server: completes the handshake then drops the TCP connection.
/// With one allowed reconnect attempt the background `async_nats` task shuts
/// down quickly, causing subsequent publishes to fail.
async fn failing_nats() -> async_nats::Client {
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
                    // Drop writer → closes TCP connection.
                    // listener drops at end of task → no reconnect possible.
                    return;
                } else if line.starts_with("PING") {
                    writer.write_all(b"PONG\r\n").await.ok();
                }
            }
        }
        // listener dropped here
    });

    // max_reconnects(1): after the connection drops, one reconnect attempt is
    // made, fails (port no longer listening), then the background task exits.
    async_nats::ConnectOptions::new()
        .max_reconnects(1_usize)
        .connect(format!("nats://127.0.0.1:{port}"))
        .await
        .unwrap()
}

/// `prompt()` publishes NATS notifications for each Codex event. When those
/// publishes fail (NATS connection dead), the errors must be logged but must
/// NOT cause `prompt()` to return `Err`. The turn must still complete normally.
#[tokio::test(flavor = "current_thread")]
async fn nats_publish_failure_during_prompt_is_non_fatal() {
    let _guard = bin_env_lock().lock().await;
    // Emit multiple text events so NATS publish is attempted several times.
    unsafe { std::env::set_var("MOCK_SEND_N_TEXT_EVENTS", "3") };
    let local = LocalSet::new();
    local
        .run_until(async {
            unsafe { std::env::set_var("CODEX_BIN", MOCK_BIN) };
            let nats = failing_nats().await;

            // Yield to let the NATS background task detect the dropped connection
            // and exhaust its single reconnect attempt before calling prompt().
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            let agent =
                DefaultCodexAgent::with_nats(nats, AcpPrefix::new("test").unwrap(), "o4-mini");

            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();

            // Even if NATS publish fails for each event, prompt() must complete.
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
    unsafe { std::env::remove_var("MOCK_SEND_N_TEXT_EVENTS") };
}

// ── cancel when process is dead ───────────────────────────────────────────────

/// `cancel()` checks `is_alive()` before calling `turn_interrupt`. When the
/// process has already exited, `is_alive()` returns false and the interrupt is
/// skipped. The session must NOT be cleared (only `process()` clears sessions
/// on death). `cancel()` must still return `Ok(())`.
#[tokio::test(flavor = "current_thread")]
async fn cancel_when_process_dead_skips_interrupt_and_returns_ok() {
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_EXIT_AFTER_TURN_ACK", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let session_id = sess.session_id.to_string();

            // Run prompt — process dies right after acking turn/start.
            agent
                .prompt(PromptRequest::new(
                    session_id.clone(),
                    vec![ContentBlock::Text(TextContent::new("die"))],
                ))
                .await
                .unwrap();

            // Process is now dead. cancel() must not call turn_interrupt (dead
            // process) and must return Ok without spawning a new process.
            agent
                .cancel(CancelNotification::new(session_id.clone()))
                .await
                .unwrap();

            // Session was NOT cleared by cancel — clearing only happens when
            // process() detects a dead process on the next spawn call.
            let list = agent
                .list_sessions(ListSessionsRequest::new())
                .await
                .unwrap();
            assert!(
                list.sessions
                    .iter()
                    .any(|s| s.session_id.to_string() == session_id),
                "session must still exist after cancel on a dead process"
            );
        })
        .await;
    unsafe { std::env::remove_var("MOCK_EXIT_AFTER_TURN_ACK") };
}

// ── broadcast channel lag in event loop ──────────────────────────────────────

/// When the mock emits more events than the broadcast channel capacity (256),
/// the oldest events are dropped and `recv()` returns `Lagged`. The `prompt()`
/// loop handles this with `continue`, so the turn must still complete normally.
#[tokio::test(flavor = "current_thread")]
async fn prompt_handles_event_channel_lag() {
    let _guard = bin_env_lock().lock().await;
    // 300 events exceeds the channel capacity of 256 — guarantees lag.
    unsafe { std::env::set_var("MOCK_SEND_N_TEXT_EVENTS", "300") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let resp = agent
                .prompt(PromptRequest::new(
                    sess.session_id.to_string(),
                    vec![ContentBlock::Text(TextContent::new("flood"))],
                ))
                .await
                .unwrap();
            assert_eq!(resp.stop_reason, StopReason::EndTurn);
        })
        .await;
    unsafe { std::env::remove_var("MOCK_SEND_N_TEXT_EVENTS") };
}

// ── NATS failure for tool events ──────────────────────────────────────────────

/// `prompt()` publishes NATS notifications for `ToolStarted` and `ToolCompleted`
/// events. When those publishes fail (NATS connection dead), the errors must be
/// logged but must NOT cause `prompt()` to return `Err`.
#[tokio::test(flavor = "current_thread")]
async fn nats_publish_failure_for_tool_events_is_non_fatal() {
    let _guard = bin_env_lock().lock().await;
    unsafe { std::env::set_var("MOCK_SEND_TOOL_EVENT", "1") };
    let local = LocalSet::new();
    local
        .run_until(async {
            unsafe { std::env::set_var("CODEX_BIN", MOCK_BIN) };
            let nats = failing_nats().await;

            // Yield to let the NATS background task notice the dropped connection.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;

            let agent =
                DefaultCodexAgent::with_nats(nats, AcpPrefix::new("test").unwrap(), "o4-mini");

            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();

            let resp = agent
                .prompt(PromptRequest::new(
                    sess.session_id.to_string(),
                    vec![ContentBlock::Text(TextContent::new("use a tool"))],
                ))
                .await
                .unwrap();

            assert_eq!(resp.stop_reason, StopReason::EndTurn);
        })
        .await;
    unsafe { std::env::remove_var("MOCK_SEND_TOOL_EVENT") };
}

// ── broadcast error (no threadId) reaches all active turns ───────────────────

/// When `read_loop` receives a notification whose `threadId` is absent/empty,
/// it must broadcast the event to ALL active turn channels (lines 346-353 of
/// process.rs), and clear all senders if the event is terminal.
///
/// Setup: two sessions run concurrent prompts. The mock acks both `turn/start`
/// requests normally, then on the second ack immediately emits an `error`
/// notification without a `threadId`. Both receivers must get the error and
/// both prompts must complete with `StopReason::EndTurn`.
#[tokio::test(flavor = "current_thread")]
async fn broadcast_error_without_thread_id_reaches_all_active_turns() {
    let _guard = bin_env_lock().lock().await;
    // Emit the broadcast error after the 2nd turn/start is acked so that both
    // turn senders are registered before the error is routed.
    unsafe { std::env::set_var("MOCK_BROADCAST_ERROR_AFTER_TURNS", "2") };
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let sess1 = agent
                .new_session(NewSessionRequest::new("/s1"))
                .await
                .unwrap();
            let sess2 = agent
                .new_session(NewSessionRequest::new("/s2"))
                .await
                .unwrap();

            // Run both prompts concurrently. The mock will broadcast an error
            // (no threadId) after the second turn/start, so both should
            // receive it and complete.
            let (r1, r2) = tokio::join!(
                agent.prompt(PromptRequest::new(
                    sess1.session_id.to_string(),
                    vec![ContentBlock::Text(TextContent::new("session 1"))],
                )),
                agent.prompt(PromptRequest::new(
                    sess2.session_id.to_string(),
                    vec![ContentBlock::Text(TextContent::new("session 2"))],
                )),
            );

            assert_eq!(r1.unwrap().stop_reason, StopReason::EndTurn);
            assert_eq!(r2.unwrap().stop_reason, StopReason::EndTurn);
        })
        .await;
    unsafe { std::env::remove_var("MOCK_BROADCAST_ERROR_AFTER_TURNS") };
}

// ── load_session happy path ───────────────────────────────────────────────────

/// `load_session` is only tested negatively in integration (after close/respawn).
/// This test verifies the happy path end-to-end: create a session via the real
/// subprocess, set a model override, then load it and verify the state.
#[tokio::test(flavor = "current_thread")]
async fn load_session_returns_correct_state_after_new_session() {
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();
            let session_id = sess.session_id.to_string();

            // Without a model override, load_session must return the agent default.
            let resp = agent
                .load_session(LoadSessionRequest::new(session_id.clone(), "/tmp"))
                .await
                .unwrap();
            let models = resp.models.expect("models must be present");
            assert_eq!(
                models.current_model_id.to_string(),
                "o4-mini",
                "should default to agent default model"
            );
            assert!(resp.modes.is_some(), "modes must be present");

            // After setting a model override, load_session must reflect it.
            agent
                .set_session_model(SetSessionModelRequest::new(session_id.clone(), "o3"))
                .await
                .unwrap();

            let resp2 = agent
                .load_session(LoadSessionRequest::new(session_id.clone(), "/tmp"))
                .await
                .unwrap();
            let models2 = resp2.models.expect("models must be present after set");
            assert_eq!(
                models2.current_model_id.to_string(),
                "o3",
                "should reflect the model override"
            );
        })
        .await;
}

// ── set_session_mode integration ──────────────────────────────────────────────

/// Codex has no named permission modes. `set_session_mode` must accept any
/// mode string and return `Ok` without touching the subprocess.
#[tokio::test(flavor = "current_thread")]
async fn set_session_mode_is_accepted_for_real_session() {
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();

            agent
                .set_session_mode(SetSessionModeRequest::new(
                    sess.session_id.to_string(),
                    "full-auto",
                ))
                .await
                .unwrap();

            // Session must still be loadable after the mode change.
            agent
                .load_session(LoadSessionRequest::new(sess.session_id.to_string(), "/tmp"))
                .await
                .unwrap();
        })
        .await;
}

// ── set_session_config_option integration ────────────────────────────────────

/// Codex has no per-session config options. `set_session_config_option` must
/// return an empty list and leave the session intact.
#[tokio::test(flavor = "current_thread")]
async fn set_session_config_option_returns_empty_and_session_survives() {
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;
            let sess = agent
                .new_session(NewSessionRequest::new("/tmp"))
                .await
                .unwrap();

            let resp = agent
                .set_session_config_option(SetSessionConfigOptionRequest::new(
                    sess.session_id.to_string(),
                    "approval-policy",
                    "never",
                ))
                .await
                .unwrap();
            assert!(
                resp.config_options.is_empty(),
                "config_options must be empty (Codex ignores config options)"
            );

            // Session must still be loadable.
            agent
                .load_session(LoadSessionRequest::new(sess.session_id.to_string(), "/tmp"))
                .await
                .unwrap();
        })
        .await;
}

// ── session branching integration ─────────────────────────────────────────────

/// fork_session records parent_session_id: list_sessions shows parentSessionId in _meta.
#[tokio::test(flavor = "current_thread")]
async fn fork_session_records_parent_session_id_in_list() {
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let src = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
            let src_id = src.session_id.to_string();

            let fork = agent
                .fork_session(ForkSessionRequest::new(src_id.clone(), "/fork"))
                .await
                .unwrap();
            let fork_id = fork.session_id.to_string();

            let list = agent
                .list_sessions(ListSessionsRequest::new())
                .await
                .unwrap();

            let fork_info = list
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == fork_id)
                .expect("fork must appear in list_sessions");

            let meta = fork_info.meta.as_ref().expect("fork must have _meta");
            assert_eq!(
                meta.get("parentSessionId").and_then(|v| v.as_str()),
                Some(src_id.as_str()),
                "fork _meta must contain parentSessionId"
            );

            // Root session must not have parent meta.
            let src_info = list
                .sessions
                .iter()
                .find(|s| s.session_id.to_string() == src_id)
                .expect("source must appear in list_sessions");
            assert!(
                src_info.meta.is_none()
                    || src_info
                        .meta
                        .as_ref()
                        .and_then(|m| m.get("parentSessionId"))
                        .is_none(),
                "root session must not have parentSessionId in _meta"
            );
        })
        .await;
}

/// ext_method("session/list_children") returns the direct children of a session.
#[tokio::test(flavor = "current_thread")]
async fn ext_list_children_returns_direct_children_integration() {
    let _guard = bin_env_lock().lock().await;
    let local = LocalSet::new();
    local
        .run_until(async {
            let agent = make_agent().await;

            let src = agent
                .new_session(NewSessionRequest::new("/src"))
                .await
                .unwrap();
            let src_id = src.session_id.to_string();

            let c1 = agent
                .fork_session(ForkSessionRequest::new(src_id.clone(), "/c1"))
                .await
                .unwrap()
                .session_id
                .to_string();
            let c2 = agent
                .fork_session(ForkSessionRequest::new(src_id.clone(), "/c2"))
                .await
                .unwrap()
                .session_id
                .to_string();

            let raw_params = serde_json::value::RawValue::from_string(
                format!(r#"{{"sessionId":"{}"}}"#, src_id),
            )
            .unwrap();
            let resp = agent
                .ext_method(ExtRequest::new("session/list_children", raw_params.into()))
                .await
                .unwrap();

            let body: serde_json::Value = serde_json::from_str(resp.0.get()).unwrap();
            let mut children: Vec<String> = body["children"]
                .as_array()
                .expect("response must have children array")
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
            children.sort();
            let mut expected = vec![c1, c2];
            expected.sort();
            assert_eq!(children, expected, "must return exactly the two direct children");
        })
        .await;
}
