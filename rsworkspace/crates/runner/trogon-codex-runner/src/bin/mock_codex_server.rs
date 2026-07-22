//! Configurable mock of `codex app-server` (protocol 0.138) for integration testing.
//!
//! Reads newline-delimited JSON-RPC from stdin, responds on stdout.
//! Behaviour is controlled by environment variables inherited from the test
//! process at spawn time:
//!
//! | Flag                             | Effect                                                    |
//! |----------------------------------|-----------------------------------------------------------|
//! | `MOCK_EXIT_AFTER_TURN_ACK`       | Exit right after acking `turn/start` (no events)         |
//! | `MOCK_HANG_AFTER_TURN_ACK`       | Ack `turn/start` then stay silent (no events, no exit)   |
//! | `MOCK_TURN_SENDS_ERROR`          | Emit `error` notification instead of `turn/completed`    |
//! | `MOCK_RESUME_FAILS`              | Return JSON-RPC error for `thread/resume`                 |
//! | `MOCK_THREAD_START_FAILS`        | Return JSON-RPC error for `thread/start`                  |
//! | `MOCK_THREAD_START_NO_ID`        | Return result with no `thread.id` for `thread/start`     |
//! | `MOCK_FORK_FAILS`                | Return JSON-RPC error for `thread/fork`                   |
//! | `MOCK_FORK_NO_ID`                | Return result with no `thread.id` for `thread/fork`      |
//! | `MOCK_TURN_START_FAILS`          | Return JSON-RPC error for `turn/start`                    |
//! | `MOCK_INITIALIZE_FAILS`          | Return JSON-RPC error for `initialize`                    |
//! | `MOCK_HANG_ON_INITIALIZE`        | Block forever on `initialize` (never respond)             |
//! | `MOCK_MALFORMED_BEFORE_COMPLETE` | Emit `{INVALID}` line before `turn/completed`             |
//! | `MOCK_UNKNOWN_ID_BEFORE_COMPLETE`| Emit response with unknown ID before `turn/completed`     |
//! | `MOCK_NONINT_ID_BEFORE_COMPLETE` | Emit response with non-integer ID before `turn/completed` |
//! | `MOCK_REQUIRE_MODEL=<id>`        | Fail `turn/start` unless `params.model` equals `<id>`     |
//! | `MOCK_INTERRUPT_FAILS`           | Return JSON-RPC error for `turn/interrupt`                 |
//! | `MOCK_SEND_N_TEXT_EVENTS=N`      | Emit N `item/agentMessage/delta` events before complete   |
//! | `MOCK_EMIT_STRAY_THREAD_EVENT`   | Emit delta for a nonexistent thread before `turn/completed` |
//! | `MOCK_SEND_TOOL_EVENT`           | Emit tool `item/started` + `item/completed` before complete |
//! | `MOCK_SEND_APPROVAL`             | Emit `execCommandApproval` server request mid-turn          |
//! | `MOCK_SEND_USAGE`                | Emit `thread/tokenUsage/updated` (default: on)            |
//! | `MOCK_SEND_REASONING`            | Emit an `item/reasoning/textDelta` before text            |
//! | `MOCK_TOOL_FAILED`               | Tool `item/completed` carries `exitCode`!=0 + failed status |
//! | `MOCK_REQUIRE_APPROVAL_POLICY=<v>` | Fail `turn/start` unless `params.approvalPolicy` equals `<v>` |
//! | `MOCK_BROADCAST_ERROR_AFTER_TURNS=N` | After N `turn/start` acks, emit broadcast `error`     |
//! | `MOCK_VALIDATE_SCHEMA`               | Reject malformed `turn/start` params                    |
//!
//! Set `CODEX_BIN` in the test process to the path of this binary so that
//! `CodexProcess::spawn()` forks this mock instead of the real CLI.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::io::{BufRead, Write};

use serde_json::Value;

fn main() {
    let exit_after_turn_ack = std::env::var("MOCK_EXIT_AFTER_TURN_ACK").is_ok();
    let hang_after_turn_ack = std::env::var("MOCK_HANG_AFTER_TURN_ACK").is_ok();
    let turn_sends_error = std::env::var("MOCK_TURN_SENDS_ERROR").is_ok();
    let resume_fails = std::env::var("MOCK_RESUME_FAILS").is_ok();
    let thread_start_fails = std::env::var("MOCK_THREAD_START_FAILS").is_ok();
    let thread_start_no_id = std::env::var("MOCK_THREAD_START_NO_ID").is_ok();
    let fork_fails = std::env::var("MOCK_FORK_FAILS").is_ok();
    let fork_no_id = std::env::var("MOCK_FORK_NO_ID").is_ok();
    let turn_start_fails = std::env::var("MOCK_TURN_START_FAILS").is_ok();
    let initialize_fails = std::env::var("MOCK_INITIALIZE_FAILS").is_ok();
    let hang_on_initialize = std::env::var("MOCK_HANG_ON_INITIALIZE").is_ok();
    let malformed_before_complete = std::env::var("MOCK_MALFORMED_BEFORE_COMPLETE").is_ok();
    let unknown_id_before_complete = std::env::var("MOCK_UNKNOWN_ID_BEFORE_COMPLETE").is_ok();
    let nonint_id_before_complete = std::env::var("MOCK_NONINT_ID_BEFORE_COMPLETE").is_ok();
    let require_model = std::env::var("MOCK_REQUIRE_MODEL").ok();
    let interrupt_fails = std::env::var("MOCK_INTERRUPT_FAILS").is_ok();
    let send_n_text_events: usize = std::env::var("MOCK_SEND_N_TEXT_EVENTS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let emit_stray_thread_event = std::env::var("MOCK_EMIT_STRAY_THREAD_EVENT").is_ok();
    let record_turn_input_file = std::env::var("MOCK_RECORD_TURN_INPUT_FILE").ok();
    let send_tool_event = std::env::var("MOCK_SEND_TOOL_EVENT").is_ok();
    let send_approval = std::env::var("MOCK_SEND_APPROVAL").is_ok();
    let send_usage = std::env::var("MOCK_SKIP_USAGE").is_err();
    let send_reasoning = std::env::var("MOCK_SEND_REASONING").is_ok();
    let tool_failed = std::env::var("MOCK_TOOL_FAILED").is_ok();
    let require_approval_policy = std::env::var("MOCK_REQUIRE_APPROVAL_POLICY").ok();
    let broadcast_error_after_turns: Option<usize> =
        std::env::var("MOCK_BROADCAST_ERROR_AFTER_TURNS")
            .ok()
            .and_then(|s| s.parse().ok());
    let validate_schema = std::env::var("MOCK_VALIDATE_SCHEMA").is_ok();

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let mut thread_counter: u64 = 0;
    let mut turn_ack_count: usize = 0;

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let msg: Value = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(_) => continue,
        };

        // Client replies to our server requests carry `id` + `result` (no `method`).
        if msg.get("id").is_some() && msg.get("method").is_none() {
            continue;
        }

        let id = match msg.get("id") {
            Some(id) if !id.is_null() => id.clone(),
            _ => continue,
        };

        let method = msg["method"].as_str().unwrap_or("");

        match method {
            "initialize" => {
                if hang_on_initialize {
                    std::thread::sleep(std::time::Duration::from_secs(300));
                } else if initialize_fails {
                    respond_error(&mut out, &id, "mock: initialize rejected");
                } else {
                    respond(
                        &mut out,
                        &id,
                        serde_json::json!({
                            "userAgent": "mock-codex/0.138",
                            "codexHome": "/tmp/mock-codex-home",
                            "platformFamily": "linux",
                            "platformOs": "linux"
                        }),
                    );
                }
            }
            "thread/start" => {
                if thread_start_fails {
                    respond_error(&mut out, &id, "mock: thread/start rejected");
                } else if thread_start_no_id {
                    respond(&mut out, &id, serde_json::json!({}));
                } else {
                    thread_counter += 1;
                    let thread_id = format!("mock-thread-{thread_counter}");
                    respond(
                        &mut out,
                        &id,
                        thread_start_response(&thread_id, "/tmp"),
                    );
                }
            }
            "thread/resume" => {
                if resume_fails {
                    respond_error(&mut out, &id, "mock: thread/resume rejected");
                } else {
                    let thread_id = msg["params"]["threadId"]
                        .as_str()
                        .unwrap_or("mock-thread-resume");
                    respond(&mut out, &id, thread_start_response(thread_id, "/tmp"));
                }
            }
            "thread/fork" => {
                if fork_fails {
                    respond_error(&mut out, &id, "mock: thread/fork rejected");
                } else if fork_no_id {
                    respond(&mut out, &id, serde_json::json!({}));
                } else {
                    thread_counter += 1;
                    let thread_id = format!("mock-fork-{thread_counter}");
                    respond(
                        &mut out,
                        &id,
                        thread_start_response(&thread_id, "/tmp"),
                    );
                }
            }
            "turn/start" => {
                if turn_start_fails {
                    respond_error(&mut out, &id, "mock: turn/start rejected");
                    continue;
                }

                if validate_schema {
                    let thread_id_ok = msg["params"]["threadId"]
                        .as_str()
                        .map(|s| !s.is_empty())
                        .unwrap_or(false);
                    let input_ok = msg["params"]["input"].is_array()
                        && msg["params"]["input"]
                            .as_array()
                            .map(|a| !a.is_empty())
                            .unwrap_or(false);
                    if !thread_id_ok {
                        respond_error(
                            &mut out,
                            &id,
                            "schema: turn/start params.threadId must be a non-empty string",
                        );
                        continue;
                    }
                    if !input_ok {
                        respond_error(
                            &mut out,
                            &id,
                            "schema: turn/start params.input must be a non-empty array",
                        );
                        continue;
                    }
                    if let Some(model_val) = msg["params"].get("model")
                        && !model_val.is_null()
                        && model_val.as_str().map(|s| s.is_empty()).unwrap_or(true)
                    {
                        respond_error(
                            &mut out,
                            &id,
                            "schema: turn/start params.model must be a non-empty string when present",
                        );
                        continue;
                    }
                }

                if let Some(expected) = &require_model {
                    let got = msg["params"]["model"].as_str().unwrap_or("");
                    if got != expected {
                        respond_error(
                            &mut out,
                            &id,
                            &format!("mock: expected model '{expected}', got '{got}'"),
                        );
                        continue;
                    }
                }

                if let Some(expected) = &require_approval_policy {
                    let got = msg["params"]["approvalPolicy"].as_str().unwrap_or("");
                    if got != expected {
                        respond_error(
                            &mut out,
                            &id,
                            &format!(
                                "mock: expected approvalPolicy '{expected}', got '{got}'"
                            ),
                        );
                        continue;
                    }
                }

                let thread_id = msg["params"]["threadId"].as_str().unwrap_or("").to_string();
                let turn_id = format!("mock-turn-{turn_ack_count}");

                if let Some(ref path) = record_turn_input_file {
                    let text = extract_input_text(&msg["params"]["input"]);
                    std::fs::write(path, text).ok();
                }

                respond(&mut out, &id, Value::Null);
                turn_ack_count += 1;

                if broadcast_error_after_turns == Some(turn_ack_count) {
                    emit(
                        &mut out,
                        "error",
                        serde_json::json!({"message": "mock broadcast error (no threadId)"}),
                    );
                    continue;
                }

                if exit_after_turn_ack {
                    return;
                }

                if hang_after_turn_ack {
                    continue;
                }

                emit_turn_sequence(
                    &mut out,
                    &thread_id,
                    &turn_id,
                    send_n_text_events,
                    send_tool_event,
                    send_approval,
                    send_usage,
                    send_reasoning,
                    tool_failed,
                    emit_stray_thread_event,
                    malformed_before_complete,
                    unknown_id_before_complete,
                    nonint_id_before_complete,
                    turn_sends_error,
                );
            }
            "turn/interrupt" if interrupt_fails => {
                respond_error(&mut out, &id, "mock: turn/interrupt rejected");
            }
            "turn/interrupt" => {
                respond(&mut out, &id, Value::Null);
            }
            _ => {
                respond(&mut out, &id, Value::Null);
            }
        }
    }
}

fn thread_start_response(thread_id: &str, cwd: &str) -> Value {
    serde_json::json!({
        "thread": {
            "id": thread_id,
            "sessionId": format!("mock-session-{thread_id}"),
            "cwd": cwd,
            "cliVersion": "0.138.0",
            "createdAt": 0_i64,
            "updatedAt": 0_i64,
            "ephemeral": false,
            "modelProvider": "openai",
            "preview": "",
            "source": "appServer",
            "status": { "type": "idle" },
            "turns": []
        },
        "approvalPolicy": "on-request",
        "approvalsReviewer": "user",
        "cwd": cwd,
        "model": "o4-mini",
        "modelProvider": "openai",
        "sandbox": { "type": "readOnly" }
    })
}

fn extract_input_text(input: &Value) -> String {
    input
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    if item.get("type").and_then(|v| v.as_str()) == Some("text") {
                        item.get("text").and_then(|v| v.as_str())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

#[allow(clippy::too_many_arguments)]
fn emit_turn_sequence(
    out: &mut impl Write,
    thread_id: &str,
    turn_id: &str,
    send_n_text_events: usize,
    send_tool_event: bool,
    send_approval: bool,
    send_usage: bool,
    send_reasoning: bool,
    tool_failed: bool,
    emit_stray_thread_event: bool,
    malformed_before_complete: bool,
    unknown_id_before_complete: bool,
    nonint_id_before_complete: bool,
    turn_sends_error: bool,
) {
    let item_id = "mock-agent-msg-1";

    emit(
        out,
        "item/started",
        serde_json::json!({
            "threadId": thread_id,
            "turnId": turn_id,
            "startedAtMs": 1_i64,
            "item": {
                "type": "agentMessage",
                "id": item_id,
                "text": ""
            }
        }),
    );

    if send_tool_event {
        emit(
            out,
            "item/started",
            serde_json::json!({
                "threadId": thread_id,
                "turnId": turn_id,
                "startedAtMs": 2_i64,
                "item": {
                    "type": "commandExecution",
                    "id": "mock-tool-1",
                    "command": "bash",
                    "commandActions": [],
                    "cwd": "/tmp",
                    "status": "inProgress"
                }
            }),
        );
    }

    if send_approval {
        request(
            out,
            9001,
            "execCommandApproval",
            serde_json::json!({
                "threadId": thread_id,
                "callId": "mock-call-1",
                "command": ["echo", "hi"],
                "conversationId": thread_id,
                "cwd": "/tmp",
                "parsedCmd": []
            }),
        );
    }

    if send_reasoning {
        emit(
            out,
            "item/reasoning/textDelta",
            serde_json::json!({
                "threadId": thread_id,
                "turnId": turn_id,
                "itemId": item_id,
                "contentIndex": 0_i64,
                "delta": "thinking..."
            }),
        );
    }

    for i in 0..send_n_text_events {
        emit(
            out,
            "item/agentMessage/delta",
            serde_json::json!({
                "threadId": thread_id,
                "turnId": turn_id,
                "itemId": item_id,
                "delta": format!("event {i}")
            }),
        );
    }

    if send_tool_event {
        // MOCK_TOOL_FAILED flips the completed item to a non-zero exit + failed
        // status so the runner exercises its Failed-status derivation (P4).
        let (status, exit_code, output) = if tool_failed {
            ("failed", 1_i64, "boom")
        } else {
            ("completed", 0_i64, "hi")
        };
        emit(
            out,
            "item/completed",
            serde_json::json!({
                "threadId": thread_id,
                "turnId": turn_id,
                "completedAtMs": 3_i64,
                "item": {
                    "type": "commandExecution",
                    "id": "mock-tool-1",
                    "command": "bash",
                    "commandActions": [],
                    "cwd": "/tmp",
                    "status": status,
                    "exitCode": exit_code,
                    "aggregatedOutput": output
                }
            }),
        );
    }

    if send_usage {
        emit(
            out,
            "thread/tokenUsage/updated",
            serde_json::json!({
                "threadId": thread_id,
                "turnId": turn_id,
                "tokenUsage": {
                    "last": {
                        "inputTokens": 42_i64,
                        "outputTokens": 17_i64,
                        "totalTokens": 59_i64,
                        "cachedInputTokens": 0_i64,
                        "reasoningOutputTokens": 0_i64
                    },
                    "total": {
                        "inputTokens": 42_i64,
                        "outputTokens": 17_i64,
                        "totalTokens": 59_i64,
                        "cachedInputTokens": 0_i64,
                        "reasoningOutputTokens": 0_i64
                    },
                    "modelContextWindow": 128000_i64
                }
            }),
        );
    }

    emit(
        out,
        "item/completed",
        serde_json::json!({
            "threadId": thread_id,
            "turnId": turn_id,
            "completedAtMs": 4_i64,
            "item": {
                "type": "agentMessage",
                "id": item_id,
                "text": "done"
            }
        }),
    );

    if emit_stray_thread_event {
        emit(
            out,
            "item/agentMessage/delta",
            serde_json::json!({
                "threadId": "nonexistent-stray-thread-9999",
                "turnId": "stray-turn",
                "itemId": "stray-item",
                "delta": "stray"
            }),
        );
    }

    if malformed_before_complete {
        writeln!(out, "{{INVALID JSON}}").unwrap();
        out.flush().unwrap();
    }

    if unknown_id_before_complete {
        let s = serde_json::json!({"id": 99999, "result": {}});
        writeln!(out, "{s}").unwrap();
        out.flush().unwrap();
    }

    if nonint_id_before_complete {
        let s = serde_json::json!({"id": "not-a-number", "result": {}});
        writeln!(out, "{s}").unwrap();
        out.flush().unwrap();
    }

    if turn_sends_error {
        emit(
            out,
            "error",
            serde_json::json!({
                "threadId": thread_id,
                "message": "mock error during turn"
            }),
        );
    } else {
        emit(
            out,
            "turn/completed",
            serde_json::json!({
                "threadId": thread_id,
                "turn": {
                    "id": turn_id,
                    "status": "completed",
                    "items": []
                }
            }),
        );
    }
}

fn respond(out: &mut impl Write, id: &Value, result: Value) {
    let resp = serde_json::json!({"id": id, "result": result});
    writeln!(out, "{resp}").unwrap();
    out.flush().unwrap();
}

fn respond_error(out: &mut impl Write, id: &Value, message: &str) {
    let resp = serde_json::json!({"id": id, "error": {"message": message}});
    writeln!(out, "{resp}").unwrap();
    out.flush().unwrap();
}

fn emit(out: &mut impl Write, method: &str, params: Value) {
    let notif = serde_json::json!({"method": method, "params": params});
    writeln!(out, "{notif}").unwrap();
    out.flush().unwrap();
}

fn request(out: &mut impl Write, id: u64, method: &str, params: Value) {
    let req = serde_json::json!({"id": id, "method": method, "params": params});
    writeln!(out, "{req}").unwrap();
    out.flush().unwrap();
}
