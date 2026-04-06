//! Configurable mock of `codex app-server` for integration testing.
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
//! | `MOCK_THREAD_START_NO_ID`        | Return result with no `threadId` field for `thread/start`|
//! | `MOCK_FORK_FAILS`                | Return JSON-RPC error for `thread/fork`                   |
//! | `MOCK_FORK_NO_ID`                | Return result with no `threadId` field for `thread/fork` |
//! | `MOCK_TURN_START_FAILS`          | Return JSON-RPC error for `turn/start`                    |
//! | `MOCK_INITIALIZE_FAILS`          | Return JSON-RPC error for `initialize`                    |
//! | `MOCK_HANG_ON_INITIALIZE`        | Block forever on `initialize` (never respond)             |
//! | `MOCK_MALFORMED_BEFORE_COMPLETE` | Emit `{INVALID}` line before `turn/completed`             |
//! | `MOCK_UNKNOWN_ID_BEFORE_COMPLETE`| Emit response with unknown ID before `turn/completed`     |
//! | `MOCK_NONINT_ID_BEFORE_COMPLETE` | Emit response with non-integer ID before `turn/completed` |
//! | `MOCK_REQUIRE_MODEL=<id>`        | Fail `turn/start` unless `params.model` equals `<id>`     |
//!
//! Set `CODEX_BIN` in the test process to the path of this binary so that
//! `CodexProcess::spawn()` forks this mock instead of the real CLI.

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

    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let mut thread_counter: u64 = 0;

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

        // Messages without an `id` are notifications — no response expected.
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
                    respond(&mut out, &id, serde_json::json!({"capabilities": {}}));
                }
            }
            "thread/start" => {
                if thread_start_fails {
                    respond_error(&mut out, &id, "mock: thread/start rejected");
                } else if thread_start_no_id {
                    // Return a result without the threadId field.
                    respond(&mut out, &id, serde_json::json!({}));
                } else {
                    thread_counter += 1;
                    respond(
                        &mut out,
                        &id,
                        serde_json::json!({"threadId": format!("mock-thread-{thread_counter}")}),
                    );
                }
            }
            "thread/resume" => {
                if resume_fails {
                    respond_error(&mut out, &id, "mock: thread/resume rejected");
                } else {
                    respond(&mut out, &id, Value::Null);
                }
            }
            "thread/fork" => {
                if fork_fails {
                    respond_error(&mut out, &id, "mock: thread/fork rejected");
                } else if fork_no_id {
                    respond(&mut out, &id, serde_json::json!({}));
                } else {
                    thread_counter += 1;
                    respond(
                        &mut out,
                        &id,
                        serde_json::json!({"threadId": format!("mock-fork-{thread_counter}")}),
                    );
                }
            }
            "turn/start" => {
                if turn_start_fails {
                    respond_error(&mut out, &id, "mock: turn/start rejected");
                    continue;
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

                let thread_id = msg["params"]["threadId"].as_str().unwrap_or("").to_string();

                // Ack the request first.
                respond(&mut out, &id, Value::Null);

                if exit_after_turn_ack {
                    return;
                }

                // Stay silent: acked but no events — lets the timeout fire.
                if hang_after_turn_ack {
                    continue;
                }

                // Emit a TextDelta so callers see at least one real event.
                emit(
                    &mut out,
                    "item/updated",
                    serde_json::json!({
                        "threadId": thread_id,
                        "item": {
                            "type": "message",
                            "content": [{"type": "output_text", "text": "hello from mock"}]
                        }
                    }),
                );

                if malformed_before_complete {
                    writeln!(&mut out, "{{INVALID JSON}}").unwrap();
                    out.flush().unwrap();
                }

                if unknown_id_before_complete {
                    let s = serde_json::json!({"id": 99999, "result": {}});
                    writeln!(&mut out, "{s}").unwrap();
                    out.flush().unwrap();
                }

                if nonint_id_before_complete {
                    let s = serde_json::json!({"id": "not-a-number", "result": {}});
                    writeln!(&mut out, "{s}").unwrap();
                    out.flush().unwrap();
                }

                if turn_sends_error {
                    emit(
                        &mut out,
                        "error",
                        serde_json::json!({
                            "threadId": thread_id,
                            "message": "mock error during turn"
                        }),
                    );
                } else {
                    emit(
                        &mut out,
                        "turn/completed",
                        serde_json::json!({
                            "threadId": thread_id
                        }),
                    );
                }
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
