mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use support::{
    FailingLauncher, FakeTransport, TestResult, doc, eventually, lock, project_dir, reply, server_for, server_with,
};
use trogon_sandbox_lsp::{
    LspError, LspPresetName, LspServer, LspServerOptions, LspServerSpec, OutputKind, Position, Signal,
};

fn symbol_server(transport: &Arc<FakeTransport>) {
    transport.respond_with(|msg| match msg["method"].as_str() {
        Some("initialize") => vec![reply(
            msg,
            json!({ "capabilities": {}, "serverInfo": { "name": "fake" } }),
        )],
        Some("shutdown") => vec![reply(msg, Value::Null)],
        Some("textDocument/documentSymbol") => {
            let range = json!({ "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } });
            vec![reply(
                msg,
                json!([{ "name": "greet", "kind": 12, "range": range, "selectionRange": range }]),
            )]
        }
        Some("textDocument/completion") => {
            if msg["params"]["position"] != json!({ "line": 5, "character": 8 }) {
                return vec![reply(msg, Value::Null)];
            }
            vec![reply(
                msg,
                json!({ "isIncomplete": false, "items": [{ "label": "length" }, { "label": "trim" }] }),
            )]
        }
        Some("custom/answer") => vec![reply(msg, json!("not a number"))],
        _ => vec![],
    });
}

#[tokio::test]
async fn start_launches_the_preset_and_handshakes() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, launcher) = server_for(&transport)?;
    let result = server.start().await?;
    assert!(result.server_info.is_none());

    let launched = launcher.launched();
    assert_eq!(launched[0].command, "typescript-language-server");
    assert_eq!(launched[0].args, vec!["--stdio"]);
    assert_eq!(launched[0].cwd.as_str(), "/workspace/project");
    assert_eq!(server.project_dir().as_str(), "/workspace/project");

    let written = transport.written();
    assert_eq!(written[0]["method"], "initialize");
    assert_eq!(written[0]["id"], 1);
    assert_eq!(written[0]["params"]["rootUri"], "file:///workspace/project");
    assert_eq!(
        written[0]["params"]["workspaceFolders"],
        json!([{ "uri": "file:///workspace/project", "name": "project" }])
    );
    assert_eq!(written[0]["params"]["processId"], Value::Null);
    assert_eq!(written[0]["params"]["initializationOptions"], Value::Null);
    assert_eq!(written[0]["params"]["capabilities"]["workspace"]["configuration"], true);
    assert_eq!(written[1]["method"], "initialized");
    assert_eq!(written[1]["params"], json!({}));
    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn extra_capabilities_merge_over_the_defaults() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let opts = LspServerOptions {
        capabilities: Some(json!({
            "textDocument": { "hover": { "contentFormat": ["plaintext"] }, "rename": {} },
            "workspace": "replaced",
        })),
        ..LspServerOptions::default()
    };
    let (server, _) = server_with(&transport, opts)?;
    transport.exit_on_eof();
    server.start().await?;
    let capabilities = &transport.written()[0]["params"]["capabilities"];
    assert_eq!(
        capabilities["textDocument"]["hover"]["contentFormat"],
        json!(["plaintext"])
    );
    assert_eq!(capabilities["textDocument"]["rename"], json!({}));
    assert_eq!(capabilities["textDocument"]["definition"]["linkSupport"], true);
    assert_eq!(capabilities["workspace"], "replaced");
    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn initialization_options_come_from_the_spec() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    transport.exit_on_eof();
    let mut spec = LspServerSpec::from(LspPresetName::Python);
    spec.initialization_options = Some(json!({ "strict": true }));
    let launcher = support::FakeLauncher::new(transport.clone());
    let server = LspServer::new(launcher, spec, project_dir()?, LspServerOptions::default());
    server.start().await?;
    assert_eq!(
        transport.written()[0]["params"]["initializationOptions"],
        json!({ "strict": true })
    );
    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn start_is_idempotent() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, launcher) = server_for(&transport)?;
    server.start().await?;
    server.start().await?;
    assert_eq!(launcher.launched().len(), 1);
    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn document_calls_use_project_relative_uris_and_versions() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    let main = doc("main.ts")?;
    assert_eq!(server.uri(&main), "file:///workspace/project/main.ts");
    server.did_open(&main, "let a", None).await?;
    server.did_change(&main, "let b").await?;
    server.did_change(&main, "let c").await?;
    server.did_close(&main).await?;
    server.did_open(&doc("notes.txt")?, "x", Some("markdown")).await?;

    let written = transport.written();
    let open = &written[2];
    assert_eq!(open["method"], "textDocument/didOpen");
    assert_eq!(
        open["params"]["textDocument"],
        json!({ "uri": "file:///workspace/project/main.ts", "languageId": "typescript", "version": 1, "text": "let a" })
    );
    assert_eq!(written[3]["params"]["textDocument"]["version"], 2);
    assert_eq!(written[3]["params"]["contentChanges"], json!([{ "text": "let b" }]));
    assert_eq!(written[4]["params"]["textDocument"]["version"], 3);
    assert_eq!(written[5]["method"], "textDocument/didClose");
    assert_eq!(
        written[5]["params"]["textDocument"]["uri"],
        "file:///workspace/project/main.ts"
    );
    assert_eq!(written[6]["params"]["textDocument"]["languageId"], "markdown");
    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_server_without_a_language_id_opens_documents_as_plaintext() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    transport.exit_on_eof();
    let launcher = support::FakeLauncher::new(transport.clone());
    let server = LspServer::new(
        launcher,
        LspServerSpec::new("custom-ls"),
        project_dir()?,
        LspServerOptions::default(),
    );
    server.start().await?;
    server.did_open(&doc("a.txt")?, "", None).await?;
    assert_eq!(
        transport.written()[2]["params"]["textDocument"]["languageId"],
        "plaintext"
    );
    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn symbols_and_completions_decode_results() -> TestResult {
    let transport = FakeTransport::new();
    symbol_server(&transport);
    let (server, _) = server_for(&transport)?;
    let info = server.start().await?.server_info.ok_or("no server info")?;
    assert_eq!(info.name, "fake");
    let main = doc("main.ts")?;
    let symbols = server.document_symbols(&main).await?.ok_or("no symbols")?;
    assert_eq!(symbols.names(), vec!["greet"]);
    let items = server.completions(&main, Position::new(5, 8)).await?;
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, vec!["length", "trim"]);
    assert!(server.completions(&main, Position::new(0, 0)).await?.is_empty());
    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_result_of_the_wrong_shape_is_a_protocol_error() -> TestResult {
    let transport = FakeTransport::new();
    symbol_server(&transport);
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    let err = server
        .request_as::<u32>("custom/answer", None, None)
        .await
        .err()
        .ok_or("decoded a string as a number")?;
    assert!(matches!(err, LspError::Protocol(m) if m.contains("unexpected custom/answer result")));
    assert_eq!(
        server.request("custom/answer", None, None).await?,
        json!("not a number")
    );
    server.notify("custom/note", Some(json!({ "a": 1 }))).await?;
    assert_eq!(transport.methods().last().map(String::as_str), Some("custom/note"));
    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn an_initialize_result_of_the_wrong_shape_fails_the_start() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_with(|msg| vec![reply(msg, json!("nope"))]);
    let (server, _) = server_for(&transport)?;
    let err = server.start().await.err().ok_or("start succeeded")?;
    assert!(matches!(err, LspError::Protocol(m) if m.contains("unexpected initialize result")));
    Ok(())
}

#[tokio::test]
async fn notifications_and_stderr_reach_listeners_registered_before_start() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    let diagnostics = Arc::new(Mutex::new(Vec::new()));
    server.on_notification("textDocument/publishDiagnostics", {
        let diagnostics = diagnostics.clone();
        move |params| lock(&diagnostics).push(params.clone())
    });
    let stderr = Arc::new(Mutex::new(String::new()));
    server.on_stderr({
        let stderr = stderr.clone();
        move |text| lock(&stderr).push_str(text)
    });
    server.on_stderr(|_| std::panic::resume_unwind(Box::new("listener bug")));
    server.start().await?;
    transport.emit(&json!({ "jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": { "n": 1 } }))?;
    transport.emit_chunk(OutputKind::Stderr, 0, b"warn ");
    transport.emit_chunk(OutputKind::Stderr, 0, b"duplicate");
    transport.emit_chunk(OutputKind::Stderr, 1, b"more");
    eventually(|| !lock(&diagnostics).is_empty() && lock(&stderr).len() == 9).await?;
    assert_eq!(lock(&diagnostics)[0], json!({ "n": 1 }));
    assert_eq!(*lock(&stderr), "warn more");
    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn stderr_text_split_inside_a_character_is_held_back() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    let stderr = Arc::new(Mutex::new(String::new()));
    server.on_stderr({
        let stderr = stderr.clone();
        move |text| lock(&stderr).push_str(text)
    });
    server.start().await?;
    let bytes = "é世".as_bytes();
    transport.emit_chunk(OutputKind::Stderr, 0, &bytes[..1]);
    transport.emit_chunk(OutputKind::Stderr, 1, &bytes[1..]);
    transport.emit_chunk(OutputKind::Stderr, 2, &[0xff, b'a']);
    eventually(|| lock(&stderr).ends_with('a')).await?;
    assert_eq!(*lock(&stderr), "é世\u{FFFD}a");
    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn a_stderr_listener_can_be_detached() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    let hits = Arc::new(AtomicUsize::new(0));
    let handle = server.on_stderr({
        let hits = hits.clone();
        move |_| {
            hits.fetch_add(1, Ordering::SeqCst);
        }
    });
    server.start().await?;
    transport.emit_chunk(OutputKind::Stderr, 0, b"a");
    eventually(|| hits.load(Ordering::SeqCst) == 1).await?;
    handle.detach();
    transport.emit_chunk(OutputKind::Stderr, 1, b"b");
    server.stop().await;
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_listener_added_after_start_can_be_detached() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    let hits = Arc::new(AtomicUsize::new(0));
    let handle = server.on_notification("x", {
        let hits = hits.clone();
        move |_| {
            hits.fetch_add(1, Ordering::SeqCst);
        }
    });
    transport.emit(&json!({ "jsonrpc": "2.0", "method": "x" }))?;
    eventually(|| hits.load(Ordering::SeqCst) == 1).await?;
    handle.detach();
    transport.emit(&json!({ "jsonrpc": "2.0", "method": "x" }))?;
    transport.emit(&json!({ "jsonrpc": "2.0", "method": "done" }))?;
    server.stop().await;
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    Ok(())
}

#[tokio::test]
async fn a_listener_detached_before_start_is_never_attached() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    let hits = Arc::new(AtomicUsize::new(0));
    let handle = server.on_notification("x", {
        let hits = hits.clone();
        move |_| {
            hits.fetch_add(1, Ordering::SeqCst);
        }
    });
    handle.detach();
    server.start().await?;
    transport.emit(&json!({ "jsonrpc": "2.0", "method": "x" }))?;
    server.stop().await;
    assert_eq!(hits.load(Ordering::SeqCst), 0);
    Ok(())
}

#[tokio::test]
async fn a_stdout_gap_fails_pending_requests_and_kills_the_server() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    let request = server.request("slow", None, None);
    let gap = async {
        eventually(|| transport.methods().contains(&"slow".to_string())).await?;
        transport.emit_chunk(OutputKind::Stdout, 9, b"x");
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let (result, gap) = tokio::join!(request, gap);
    gap?;
    let err = result.err().ok_or("request succeeded")?;
    assert!(matches!(err, LspError::Protocol(m) if m.contains("missing stdout seq 1 (received seq 9)")));
    eventually(|| transport.kills() == vec![Signal::Kill]).await?;
    let later = server
        .request("later", None, None)
        .await
        .err()
        .ok_or("request succeeded")?;
    assert!(matches!(later, LspError::NotRunning(_)));
    Ok(())
}

#[tokio::test]
async fn a_malformed_frame_fails_the_connection() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    let request = server.request("slow", None, None);
    let garbage = async {
        eventually(|| transport.methods().contains(&"slow".to_string())).await?;
        transport.emit_raw(b"Content-Length: 3\r\n\r\n{x}");
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let (result, garbage) = tokio::join!(request, garbage);
    garbage?;
    assert!(matches!(result, Err(LspError::Protocol(m)) if m.contains("JSON")));
    eventually(|| transport.kills() == vec![Signal::Kill]).await?;
    Ok(())
}

#[tokio::test]
async fn an_output_error_fails_pending_requests() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    let request = server.request("slow", None, None);
    let failure = async {
        eventually(|| transport.methods().contains(&"slow".to_string())).await?;
        transport.emit_error(LspError::Transport("stream broke".into()));
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let (result, failure) = tokio::join!(request, failure);
    failure?;
    assert!(matches!(result, Err(LspError::Transport(m)) if m == "stream broke"));
    Ok(())
}

#[tokio::test]
async fn output_ending_fails_pending_requests_as_not_running() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    let request = server.request("slow", None, None);
    let end = async {
        eventually(|| transport.methods().contains(&"slow".to_string())).await?;
        transport.close_output();
        Ok::<(), Box<dyn std::error::Error>>(())
    };
    let (result, end) = tokio::join!(request, end);
    end?;
    assert!(matches!(result, Err(LspError::NotRunning(m)) if m.contains("output stream ended")));
    Ok(())
}

#[tokio::test]
async fn start_failure_kills_and_reports_the_error() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_with(|msg| {
        vec![json!({ "jsonrpc": "2.0", "id": msg["id"], "error": { "code": -32603, "message": "init failed" } })]
    });
    let (server, _) = server_for(&transport)?;
    let err = server.start().await.err().ok_or("start succeeded")?;
    assert!(matches!(err, LspError::Response { code: -32603, .. }));
    assert_eq!(transport.kills(), vec![Signal::Kill]);
    let again = server.start().await.err().ok_or("second start succeeded")?;
    assert!(matches!(again, LspError::NotRunning(_)));
    Ok(())
}

#[tokio::test]
async fn a_launch_failure_is_reported_and_leaves_nothing_to_kill() -> TestResult {
    let server = LspServer::new(
        Arc::new(FailingLauncher),
        LspPresetName::Python,
        project_dir()?,
        LspServerOptions::default(),
    );
    let err = server.start().await.err().ok_or("start succeeded")?;
    assert!(matches!(err, LspError::Transport(m) if m == "cannot launch"));
    Ok(())
}

#[tokio::test]
async fn calls_before_start_are_not_running() -> TestResult {
    let transport = FakeTransport::new();
    let (server, _) = server_for(&transport)?;
    let main = doc("a.ts")?;
    assert!(matches!(server.did_open(&main, "", None).await, Err(LspError::NotRunning(m)) if m.contains("Idle")));
    assert!(matches!(
        server.did_change(&main, "").await,
        Err(LspError::NotRunning(_))
    ));
    assert!(matches!(server.did_close(&main).await, Err(LspError::NotRunning(_))));
    assert!(matches!(server.notify("x", None).await, Err(LspError::NotRunning(_))));
    assert!(matches!(
        server.document_symbols(&main).await,
        Err(LspError::NotRunning(_))
    ));
    assert!(matches!(
        server.completions(&main, Position::new(0, 0)).await,
        Err(LspError::NotRunning(_))
    ));
    Ok(())
}

#[tokio::test]
async fn a_launcher_whose_output_was_already_taken_fails_the_start() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let _taken = trogon_sandbox_lsp::LspTransport::output(transport.as_ref())?;
    let (server, _) = server_for(&transport)?;
    let err = server.start().await.err().ok_or("start succeeded")?;
    assert!(matches!(err, LspError::Transport(m) if m.contains("already taken")));
    Ok(())
}

#[tokio::test]
async fn dropping_the_server_stops_its_background_tasks() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    drop(server);
    transport.emit_chunk(OutputKind::Stderr, 0, b"ignored");
    tokio::task::yield_now().await;
    Ok(())
}

#[tokio::test]
async fn non_object_capabilities_replace_the_defaults() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let opts = LspServerOptions {
        capabilities: Some(json!("none")),
        ..LspServerOptions::default()
    };
    let (server, _) = server_with(&transport, opts)?;
    transport.exit_on_eof();
    server.start().await?;
    assert_eq!(transport.written()[0]["params"]["capabilities"], "none");
    server.stop().await;
    Ok(())
}

#[tokio::test]
async fn did_change_on_a_document_that_was_never_opened_is_not_open() -> TestResult {
    let transport = FakeTransport::new();
    transport.respond_to_lifecycle();
    let (server, _) = server_for(&transport)?;
    server.start().await?;
    let main = doc("main.ts")?;
    let before = transport.written().len();
    assert!(matches!(
        server.did_change(&main, "x").await,
        Err(LspError::NotOpen { uri }) if uri == "file:///workspace/project/main.ts"
    ));
    server.did_open(&main, "a", None).await?;
    server.did_close(&main).await?;
    assert!(matches!(
        server.did_change(&main, "b").await,
        Err(LspError::NotOpen { .. })
    ));
    assert_eq!(transport.written().len(), before + 2);
    server.stop().await;
    Ok(())
}
