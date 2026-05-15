//! Live end-to-end test for trogon-xai-runner coding tools.
//!
//! Wires XaiAgent through AgentSideNatsConnection using real NATS
//! (localhost:4222) and MockXaiHttpClient to control xAI responses.
//! Tool execution hits the real filesystem / git / HTTP.
//!
//! Requirements:
//!   • NATS running on nats://localhost:4222
//!
//! Run:
//!   cargo run -p trogon-e2e --bin xai_live

use std::sync::Arc;
use std::time::Duration;

use acp_nats::acp_prefix::AcpPrefix;
use acp_nats_agent::AgentSideNatsConnection;
use agent_client_protocol::{ContentBlock, NewSessionRequest, PromptRequest};
use bytes::Bytes;
use httpmock::prelude::*;
use trogon_xai_runner::{
    FinishReason, InputItem, MockXaiHttpClient, NatsSessionNotifier, XaiAgent, XaiEvent,
};

// ── NATS helpers ──────────────────────────────────────────────────────────────

async fn connect() -> async_nats::Client {
    async_nats::connect("nats://localhost:4222")
        .await
        .expect("NATS must be running on localhost:4222")
}

async fn nats_req(nats: &async_nats::Client, subject: &str, payload: Bytes) -> serde_json::Value {
    let reply = tokio::time::timeout(Duration::from_secs(5), nats.request(subject.to_string(), payload))
        .await
        .expect("NATS request timed out")
        .expect("NATS request failed");
    serde_json::from_slice(&reply.payload).expect("response is not valid JSON")
}

async fn new_session(nats: &async_nats::Client, prefix: &str, cwd: &str) -> String {
    let subject = format!("{prefix}.agent.session.new");
    let raw = Bytes::from(serde_json::to_vec(&NewSessionRequest::new(cwd)).unwrap());
    let val = nats_req(nats, &subject, raw).await;
    val["sessionId"].as_str().expect("sessionId missing").to_string()
}

async fn prompt(nats: &async_nats::Client, prefix: &str, sid: &str, text: &str) -> serde_json::Value {
    let subject = format!("{prefix}.session.{sid}.agent.prompt");
    let raw = Bytes::from(
        serde_json::to_vec(&PromptRequest::new(sid.to_string(), vec![ContentBlock::from(text)])).unwrap(),
    );
    nats_req(nats, &subject, raw).await
}

// ── Agent factory ─────────────────────────────────────────────────────────────

struct Harness {
    nats: async_nats::Client,
    http: Arc<MockXaiHttpClient>,
    prefix: String,
}

async fn start_harness(prefix: &str) -> Harness {
    let nats = connect().await;
    let acp_prefix = AcpPrefix::new(prefix).unwrap();

    let http = Arc::new(MockXaiHttpClient::new());
    let notifier = NatsSessionNotifier::new(nats.clone(), acp_prefix.clone());
    let agent = XaiAgent::with_deps(notifier, "grok-3", "live-test-key", Arc::clone(&http));

    let (_, io_task) = AgentSideNatsConnection::new(
        agent,
        nats.clone(),
        acp_prefix,
        |fut| { tokio::task::spawn_local(fut); },
    );
    tokio::task::spawn_local(async move { io_task.await.ok(); });

    tokio::time::sleep(Duration::from_millis(100)).await;

    Harness { nats, http, prefix: prefix.to_string() }
}

// ── XaiEvent response builders ────────────────────────────────────────────────

fn text_response(id: &str, text: &str) -> Vec<XaiEvent> {
    vec![
        XaiEvent::ResponseId { id: id.to_string() },
        XaiEvent::TextDelta { text: text.to_string() },
        XaiEvent::Done,
    ]
}

fn tool_call_response(id: &str, call_id: &str, name: &str, args: &str) -> Vec<XaiEvent> {
    vec![
        XaiEvent::ResponseId { id: id.to_string() },
        XaiEvent::FunctionCall {
            call_id: call_id.to_string(),
            name: name.to_string(),
            arguments: args.to_string(),
        },
        XaiEvent::Finished { reason: FinishReason::ToolCalls, incomplete_reason: None },
        XaiEvent::Done,
    ]
}

fn tool_output<'a>(calls: &'a [trogon_xai_runner::MockCall], call_idx: usize, call_id: &str) -> Option<&'a str> {
    calls[call_idx].input.iter().find_map(|item| {
        if let InputItem::FunctionCallOutput { call_id: cid, output, .. } = item {
            if cid == call_id { Some(output.as_str()) } else { None }
        } else {
            None
        }
    })
}

// ── Tests ─────────────────────────────────────────────────────────────────────

async fn test_trogon_tools_present_in_request(h: &Harness) {
    print!("  [1] trogon-tools present in HTTP request by default ... ");

    h.http.push_response(text_response("r1", "ok"));

    let sid = new_session(&h.nats, &h.prefix, "/tmp").await;
    prompt(&h.nats, &h.prefix, &sid, "hello").await;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let calls = h.http.calls.lock().unwrap();
    assert!(!calls.is_empty(), "no HTTP call was made");
    let tools: Vec<_> = calls[0].tools.iter().map(|t| t.name().to_string()).collect();
    for expected in &["read_file", "write_file", "str_replace", "search_files", "git_status"] {
        assert!(tools.contains(&expected.to_string()), "{expected} missing from tools: {tools:?}");
    }
    println!("ok");
}

async fn test_read_file_relative_path(h: &Harness) {
    print!("  [2] read_file with relative path resolves against session cwd ... ");

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("live_marker.txt"), "live-content-xyz\n").unwrap();
    let cwd = dir.path().to_str().unwrap().to_string();

    h.http.push_response(tool_call_response(
        "r-rf", "call-rf", "read_file", r#"{"path":"live_marker.txt"}"#,
    ));
    h.http.push_response(text_response("r-rf2", "done"));

    let sid = new_session(&h.nats, &h.prefix, &cwd).await;
    prompt(&h.nats, &h.prefix, &sid, "read the marker").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let calls = h.http.calls.lock().unwrap();
    assert!(calls.len() >= 2, "expected at least 2 HTTP calls, got {}", calls.len());
    let out = tool_output(&calls, 1, "call-rf");
    assert!(out.is_some(), "follow-up must contain FunctionCallOutput for call-rf");
    assert!(out.unwrap().contains("live-content-xyz"), "output must contain file content, got: {:?}", out);
    println!("ok");
}

async fn test_search_files_finds_content(h: &Harness) {
    print!("  [3] search_files finds pattern in real filesystem ... ");

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("target.rs"), "fn live_needle_fn() {}\n").unwrap();
    std::fs::write(dir.path().join("other.rs"), "fn unrelated() {}\n").unwrap();
    let cwd = dir.path().to_str().unwrap().to_string();

    h.http.push_response(tool_call_response(
        "r-sf", "call-sf", "search_files", r#"{"pattern":"live_needle_fn"}"#,
    ));
    h.http.push_response(text_response("r-sf2", "found it"));

    let sid = new_session(&h.nats, &h.prefix, &cwd).await;
    prompt(&h.nats, &h.prefix, &sid, "search for live_needle_fn").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let calls = h.http.calls.lock().unwrap();
    assert!(calls.len() >= 2, "expected at least 2 HTTP calls, got {}", calls.len());
    let out = tool_output(&calls, 1, "call-sf");
    assert!(out.is_some(), "follow-up must contain FunctionCallOutput for call-sf");
    assert!(
        out.unwrap().contains("target.rs") && out.unwrap().contains("live_needle_fn"),
        "output must contain the match, got: {:?}", out
    );
    println!("ok");
}

async fn test_fetch_url_egress_blocked(h: &Harness) {
    print!("  [4] fetch_url to metadata endpoint blocked by egress policy ... ");

    h.http.push_response(tool_call_response(
        "r-eg", "call-eg", "fetch_url",
        r#"{"url":"http://169.254.169.254/latest/meta-data/"}"#,
    ));
    h.http.push_response(text_response("r-eg2", "blocked"));

    let sid = new_session(&h.nats, &h.prefix, "/tmp").await;
    prompt(&h.nats, &h.prefix, &sid, "fetch the metadata endpoint").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let calls = h.http.calls.lock().unwrap();
    assert!(calls.len() >= 2, "expected at least 2 HTTP calls, got {}", calls.len());
    let out = tool_output(&calls, 1, "call-eg");
    assert!(out.is_some(), "follow-up must contain FunctionCallOutput for call-eg");
    assert!(out.unwrap().contains("blocked by egress policy"), "output must contain egress error, got: {:?}", out);
    println!("ok");
}

async fn test_write_file_creates_file(h: &Harness) {
    print!("  [5] write_file creates a file on the real filesystem ... ");

    let dir = tempfile::TempDir::new().unwrap();
    let cwd = dir.path().to_str().unwrap().to_string();
    let target = dir.path().join("written.txt");

    h.http.push_response(tool_call_response(
        "r-wf", "call-wf", "write_file",
        r#"{"path":"written.txt","content":"hello from xai-runner\n"}"#,
    ));
    h.http.push_response(text_response("r-wf2", "done"));

    let sid = new_session(&h.nats, &h.prefix, &cwd).await;
    prompt(&h.nats, &h.prefix, &sid, "write a file").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let calls = h.http.calls.lock().unwrap();
    assert!(calls.len() >= 2, "expected at least 2 HTTP calls, got {}", calls.len());
    let out = tool_output(&calls, 1, "call-wf");
    assert!(out.is_some(), "follow-up must contain FunctionCallOutput for call-wf");
    assert!(target.exists(), "write_file must create the file on disk");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "hello from xai-runner\n",
        "file content must match what was written"
    );
    println!("ok");
}

async fn test_str_replace_edits_file(h: &Harness) {
    print!("  [6] str_replace applies an in-place edit to a real file ... ");

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("edit_me.txt"), "alpha beta gamma\n").unwrap();
    let cwd = dir.path().to_str().unwrap().to_string();

    h.http.push_response(tool_call_response(
        "r-sr", "call-sr", "str_replace",
        r#"{"path":"edit_me.txt","old_str":"beta","new_str":"BETA"}"#,
    ));
    h.http.push_response(text_response("r-sr2", "done"));

    let sid = new_session(&h.nats, &h.prefix, &cwd).await;
    prompt(&h.nats, &h.prefix, &sid, "edit the file").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let calls = h.http.calls.lock().unwrap();
    assert!(calls.len() >= 2, "expected at least 2 HTTP calls, got {}", calls.len());
    let out = tool_output(&calls, 1, "call-sr");
    assert!(out.is_some(), "follow-up must contain FunctionCallOutput for call-sr");
    let content = std::fs::read_to_string(dir.path().join("edit_me.txt")).unwrap();
    assert_eq!(content, "alpha BETA gamma\n", "str_replace must modify the file in place");
    println!("ok");
}

async fn test_git_status_in_real_repo(h: &Harness) {
    print!("  [7] git_status returns output for a real git repo ... ");

    let dir = tempfile::TempDir::new().unwrap();
    std::process::Command::new("git")
        .args(["init", "-b", "main"])
        .current_dir(dir.path())
        .output()
        .unwrap();
    std::fs::write(dir.path().join("untracked.rs"), "fn main() {}\n").unwrap();
    let cwd = dir.path().to_str().unwrap().to_string();

    h.http.push_response(tool_call_response("r-gs", "call-gs", "git_status", "{}"));
    h.http.push_response(text_response("r-gs2", "done"));

    let sid = new_session(&h.nats, &h.prefix, &cwd).await;
    prompt(&h.nats, &h.prefix, &sid, "what is the git status?").await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let calls = h.http.calls.lock().unwrap();
    assert!(calls.len() >= 2, "expected at least 2 HTTP calls, got {}", calls.len());
    let out = tool_output(&calls, 1, "call-gs");
    assert!(out.is_some(), "follow-up must contain FunctionCallOutput for call-gs");
    assert!(
        out.unwrap().contains("untracked.rs"),
        "git_status output must include the untracked file, got: {:?}", out
    );
    println!("ok");
}

async fn test_fetch_url_allowed_local_server(h: &Harness) {
    print!("  [8] fetch_url to an allowed local URL returns the response body ... ");

    let server = MockServer::start();
    server.mock(|when, then| {
        when.method(GET).path("/hello");
        then.status(200).header("Content-Type", "text/plain").body("live-fetch-response");
    });
    let url = server.url("/hello");

    let args = serde_json::json!({"url": url}).to_string();
    h.http.push_response(tool_call_response("r-fu", "call-fu", "fetch_url", &args));
    h.http.push_response(text_response("r-fu2", "done"));

    let sid = new_session(&h.nats, &h.prefix, "/tmp").await;
    prompt(&h.nats, &h.prefix, &sid, "fetch the local URL").await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let calls = h.http.calls.lock().unwrap();
    assert!(calls.len() >= 2, "expected at least 2 HTTP calls, got {}", calls.len());
    let out = tool_output(&calls, 1, "call-fu");
    assert!(out.is_some(), "follow-up must contain FunctionCallOutput for call-fu");
    assert!(
        out.unwrap().contains("live-fetch-response"),
        "output must contain the response body, got: {:?}", out
    );
    println!("ok");
}

async fn test_list_dir_returns_entries(h: &Harness) {
    print!("  [9] list_dir returns directory entries ... ");

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("alpha.rs"), "").unwrap();
    std::fs::write(dir.path().join("beta.rs"), "").unwrap();
    std::fs::create_dir(dir.path().join("sub")).unwrap();
    let cwd = dir.path().to_str().unwrap().to_string();

    h.http.push_response(tool_call_response("r-ld", "call-ld", "list_dir", "{}"));
    h.http.push_response(text_response("r-ld2", "done"));

    let sid = new_session(&h.nats, &h.prefix, &cwd).await;
    prompt(&h.nats, &h.prefix, &sid, "list the directory").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let calls = h.http.calls.lock().unwrap();
    assert!(calls.len() >= 2, "expected at least 2 HTTP calls, got {}", calls.len());
    let out = tool_output(&calls, 1, "call-ld");
    assert!(out.is_some(), "follow-up must contain FunctionCallOutput for call-ld");
    let text = out.unwrap();
    assert!(text.contains("alpha.rs"), "list_dir must include alpha.rs, got: {text:?}");
    assert!(text.contains("beta.rs"), "list_dir must include beta.rs, got: {text:?}");
    assert!(text.contains("sub"), "list_dir must include sub/, got: {text:?}");
    println!("ok");
}

async fn test_glob_finds_files_by_pattern(h: &Harness) {
    print!("  [10] glob finds files matching a pattern ... ");

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("main.rs"), "").unwrap();
    std::fs::write(dir.path().join("lib.rs"), "").unwrap();
    std::fs::write(dir.path().join("config.toml"), "").unwrap();
    let cwd = dir.path().to_str().unwrap().to_string();

    h.http.push_response(tool_call_response(
        "r-gl", "call-gl", "glob", r#"{"pattern":"*.rs"}"#,
    ));
    h.http.push_response(text_response("r-gl2", "done"));

    let sid = new_session(&h.nats, &h.prefix, &cwd).await;
    prompt(&h.nats, &h.prefix, &sid, "find all rust files").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let calls = h.http.calls.lock().unwrap();
    assert!(calls.len() >= 2, "expected at least 2 HTTP calls, got {}", calls.len());
    let out = tool_output(&calls, 1, "call-gl");
    assert!(out.is_some(), "follow-up must contain FunctionCallOutput for call-gl");
    let text = out.unwrap();
    assert!(text.contains("main.rs"), "glob must match main.rs, got: {text:?}");
    assert!(text.contains("lib.rs"), "glob must match lib.rs, got: {text:?}");
    assert!(!text.contains("config.toml"), "glob must not match .toml, got: {text:?}");
    println!("ok");
}

async fn test_git_diff_shows_unstaged_changes(h: &Harness) {
    print!("  [11] git_diff shows unstaged changes in the working tree ... ");

    let dir = tempfile::TempDir::new().unwrap();
    std::process::Command::new("git").args(["init", "-b", "main"]).current_dir(dir.path()).output().unwrap();
    std::process::Command::new("git").args(["config", "user.email", "test@test"]).current_dir(dir.path()).output().unwrap();
    std::process::Command::new("git").args(["config", "user.name", "Test"]).current_dir(dir.path()).output().unwrap();
    std::fs::write(dir.path().join("file.rs"), "fn old() {}\n").unwrap();
    std::process::Command::new("git").args(["add", "."]).current_dir(dir.path()).output().unwrap();
    std::process::Command::new("git").args(["commit", "-m", "init"]).current_dir(dir.path()).output().unwrap();
    std::fs::write(dir.path().join("file.rs"), "fn new() {}\n").unwrap();
    let cwd = dir.path().to_str().unwrap().to_string();

    h.http.push_response(tool_call_response("r-gd", "call-gd", "git_diff", "{}"));
    h.http.push_response(text_response("r-gd2", "done"));

    let sid = new_session(&h.nats, &h.prefix, &cwd).await;
    prompt(&h.nats, &h.prefix, &sid, "show the diff").await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let calls = h.http.calls.lock().unwrap();
    assert!(calls.len() >= 2, "expected at least 2 HTTP calls, got {}", calls.len());
    let out = tool_output(&calls, 1, "call-gd");
    assert!(out.is_some(), "follow-up must contain FunctionCallOutput for call-gd");
    let text = out.unwrap();
    assert!(text.contains("old") && text.contains("new"), "git_diff must show changed lines, got: {text:?}");
    println!("ok");
}

async fn test_git_log_shows_commits(h: &Harness) {
    print!("  [12] git_log shows commit history ... ");

    let dir = tempfile::TempDir::new().unwrap();
    std::process::Command::new("git").args(["init", "-b", "main"]).current_dir(dir.path()).output().unwrap();
    std::process::Command::new("git").args(["config", "user.email", "test@test"]).current_dir(dir.path()).output().unwrap();
    std::process::Command::new("git").args(["config", "user.name", "Test"]).current_dir(dir.path()).output().unwrap();
    std::fs::write(dir.path().join("f.rs"), "").unwrap();
    std::process::Command::new("git").args(["add", "."]).current_dir(dir.path()).output().unwrap();
    std::process::Command::new("git").args(["commit", "-m", "live-log-commit-marker"]).current_dir(dir.path()).output().unwrap();
    let cwd = dir.path().to_str().unwrap().to_string();

    h.http.push_response(tool_call_response("r-glog", "call-glog", "git_log", "{}"));
    h.http.push_response(text_response("r-glog2", "done"));

    let sid = new_session(&h.nats, &h.prefix, &cwd).await;
    prompt(&h.nats, &h.prefix, &sid, "show the git log").await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    let calls = h.http.calls.lock().unwrap();
    assert!(calls.len() >= 2, "expected at least 2 HTTP calls, got {}", calls.len());
    let out = tool_output(&calls, 1, "call-glog");
    assert!(out.is_some(), "follow-up must contain FunctionCallOutput for call-glog");
    assert!(
        out.unwrap().contains("live-log-commit-marker"),
        "git_log must include the commit message, got: {:?}", out
    );
    println!("ok");
}

async fn test_multi_turn_uses_previous_response_id(h: &Harness) {
    print!("  [13] multi-turn: turn 2 references turn 1 via previous_response_id ... ");

    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("note.txt"), "turn-one-content\n").unwrap();
    let cwd = dir.path().to_str().unwrap().to_string();

    // Turn 1: tool call + text reply — the reply's ResponseId becomes last_response_id
    h.http.push_response(tool_call_response(
        "r-mt1", "call-mt1", "read_file", r#"{"path":"note.txt"}"#,
    ));
    h.http.push_response(text_response("r-mt1b", "I read the file"));

    let sid = new_session(&h.nats, &h.prefix, &cwd).await;
    prompt(&h.nats, &h.prefix, &sid, "read note.txt").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Turn 2: the agent must hand off previous_response_id = "r-mt1b" to the HTTP client
    h.http.push_response(text_response("r-mt2", "got it"));
    prompt(&h.nats, &h.prefix, &sid, "what did the file say?").await;
    tokio::time::sleep(Duration::from_millis(200)).await;

    let calls = h.http.calls.lock().unwrap();
    assert!(calls.len() >= 3, "expected at least 3 HTTP calls (turn1 tool, turn1 reply, turn2), got {}", calls.len());
    assert_eq!(
        calls[2].previous_response_id.as_deref(),
        Some("r-mt1b"),
        "turn 2 must carry previous_response_id from the last turn-1 response"
    );
    // Only the new user message goes in input — history stays server-side via the ID
    let has_only_new_msg = calls[2].input.len() == 1
        && matches!(&calls[2].input[0], InputItem::Message { role, .. } if role == "user");
    assert!(has_only_new_msg, "turn 2 input must be just the new user message, got: {:?}", calls[2].input);
    println!("ok");
}

// ── Main ──────────────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    println!("xai-runner live test — coding tools");
    println!("====================================");

    let local = tokio::task::LocalSet::new();
    local.run_until(async {
        { let h = start_harness("xai.live.t1").await;  test_trogon_tools_present_in_request(&h).await; }
        { let h = start_harness("xai.live.t2").await;  test_read_file_relative_path(&h).await; }
        { let h = start_harness("xai.live.t3").await;  test_search_files_finds_content(&h).await; }
        { let h = start_harness("xai.live.t4").await;  test_fetch_url_egress_blocked(&h).await; }
        { let h = start_harness("xai.live.t5").await;  test_write_file_creates_file(&h).await; }
        { let h = start_harness("xai.live.t6").await;  test_str_replace_edits_file(&h).await; }
        { let h = start_harness("xai.live.t7").await;  test_git_status_in_real_repo(&h).await; }
        { let h = start_harness("xai.live.t8").await;  test_fetch_url_allowed_local_server(&h).await; }
        { let h = start_harness("xai.live.t9").await;  test_list_dir_returns_entries(&h).await; }
        { let h = start_harness("xai.live.t10").await; test_glob_finds_files_by_pattern(&h).await; }
        { let h = start_harness("xai.live.t11").await; test_git_diff_shows_unstaged_changes(&h).await; }
        { let h = start_harness("xai.live.t12").await; test_git_log_shows_commits(&h).await; }
        { let h = start_harness("xai.live.t13").await; test_multi_turn_uses_previous_response_id(&h).await; }

        println!("====================================");
        println!("all 13 live tests passed");
    }).await;
}
