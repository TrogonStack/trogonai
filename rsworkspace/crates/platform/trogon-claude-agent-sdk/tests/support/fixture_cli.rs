use std::{
    error::Error,
    io::{self, BufRead, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};

fn emit(value: Value) -> io::Result<()> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, &value)?;
    stdout.write_all(b"\n")?;
    stdout.flush()
}

fn field<'a>(value: &'a Value, key: &str) -> io::Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| io::Error::other("fixture expected string field"))
}

fn result(text: &str) -> Value {
    json!({"type":"result","subtype":"success","is_error":false,"duration_ms":1,"duration_api_ms":1,"num_turns":1,"session_id":"fixture-session","result":text,"total_cost_usd":0.0})
}

fn assistant(text: &str) -> Value {
    json!({"type":"assistant","message":{"role":"assistant","model":"fixture-model","content":[{"type":"text","text":text}]},"session_id":"fixture-session"})
}

fn acknowledge(id: &str) -> io::Result<()> {
    emit(json!({"type":"control_response","response":{"subtype":"success","request_id":id,"response":{}}}))
}

fn mcp_step(
    lines: &mut impl Iterator<Item = io::Result<String>>,
    id: &str,
    message: Value,
) -> Result<Value, Box<dyn Error>> {
    emit(
        json!({"type":"control_request","request_id":id,"request":{"subtype":"mcp_message","server_name":"local","message":message}}),
    )?;
    let response: Value =
        serde_json::from_str(&lines.next().ok_or_else(|| io::Error::other("missing MCP response"))??)?;
    assert_eq!(response["response"]["request_id"], id);
    assert_eq!(response["response"]["subtype"], "success");
    Ok(response["response"]["response"]["mcp_response"].clone())
}

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        std_env_access,
        reason = "This subprocess fixture observes the environment supplied by the SDK under test."
    )
)]
fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    assert!(args.windows(2).any(|pair| pair == ["--output-format", "stream-json"]));
    assert!(args.windows(2).any(|pair| pair == ["--input-format", "stream-json"]));
    assert!(args.iter().any(|arg| arg == "--verbose"));
    let mode = args
        .iter()
        .find_map(|arg| arg.strip_prefix("--model=fixture:"))
        .or_else(|| {
            args.windows(2).find_map(|pair| {
                (pair[0] == "--model")
                    .then(|| pair[1].strip_prefix("fixture:"))
                    .flatten()
            })
        })
        .unwrap_or("normal");
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();
    let first = lines
        .next()
        .ok_or_else(|| io::Error::other("fixture missing initialize"))??;
    let initialize: Value = serde_json::from_str(&first)?;
    // The TypeScript argv shape delivers the system prompt inside `initialize`.
    let custom_prompt = initialize["request"]["systemPrompt"][0]
        .as_str()
        .filter(|prompt| !prompt.is_empty());
    assert_eq!(initialize["type"], "control_request");
    assert_eq!(initialize["request"]["subtype"], "initialize");
    if !mode.starts_with("hook") {
        assert_eq!(initialize["request"]["hooks"], Value::Null);
    }
    let initialize_id = field(&initialize, "request_id")?;
    if mode == "noisy-stdout" {
        io::stdout().write_all(b"fixture banner\n\n   \n")?;
        io::stdout().flush()?;
    }
    if mode == "exit-before-init" {
        std::process::exit(12);
    }
    if mode == "init-timeout" {
        if let Some(path) = custom_prompt {
            let destination = std::path::Path::new(path);
            let pending = destination.with_extension("pending");
            std::fs::write(&pending, std::process::id().to_string())?;
            std::fs::rename(pending, destination)?;
        }
        std::thread::sleep(Duration::from_secs(30));
        return Ok(());
    }
    let initialization = if mode == "hook" {
        json!({"hooks_applied":true,"hook_on_failure_supported":true,"claude_code_version":"2.1.319"})
    } else if mode == "hook-not-applied" {
        json!({"hooks_applied":false,"hook_on_failure_supported":true})
    } else if mode == "init-malformed" {
        json!(42)
    } else if mode == "init-malformed-known" {
        json!({"models":"invalid"})
    } else if mode == "connection-info" {
        json!({
            "commands":[{"name":"review"}],
            "models":[{"value":"fixture-model"}],
            "agents":[{"name":"reviewer"}],
            "account":{"organization":"fixture"},
            "hooks_applied":null,
            "hook_on_failure_supported":true,
            "claude_code_version":"2.1.319",
            "future_metadata":{"nested":[1,null]},
        })
    } else {
        json!({})
    };
    emit(
        json!({"type":"control_response","response":{"subtype":"success","request_id":initialize_id,"response":initialization}}),
    )?;
    emit(
        json!({"type":"system","subtype":"init","session_id":"fixture-session","arguments":args,"cwd":std::env::current_dir()?,"initialize_request":initialize["request"],"config_dir":std::env::var_os("CLAUDE_CONFIG_DIR")}),
    )?;

    let stop = Arc::new(AtomicBool::new(false));
    let mut flood = None;
    let mut input_count = 0;
    let mut configured_servers: Vec<String> = Vec::new();
    while let Some(line) = lines.next() {
        let value: Value = serde_json::from_str(&line?)?;
        match field(&value, "type")? {
            "user" => {
                assert_eq!(value["message"]["role"], "user");
                assert_eq!(value["session_id"], "");
                assert_eq!(value["parent_tool_use_id"], Value::Null);
                input_count += 1;
                if mode == "structured" {
                    emit(json!({"type":"system","subtype":"input_observed","input":value}))?;
                    emit(result("structured"))?;
                    continue;
                }
                if mode == "queued-coalesce" {
                    if input_count == 2 {
                        let mut output = result("coalesced");
                        output["queued_turn_count"] = json!(0);
                        emit(output)?;
                        emit(json!({"type":"system","subtype":"session_state_changed","state":"idle"}))?;
                    }
                    continue;
                }
                let prompt = field(&value["message"], "content")?;
                match mode {
                    "normal" | "argv" | "connection-info" => {
                        emit(assistant(prompt))?;
                        emit(result(prompt))?;
                    }
                    "noisy-stdout" => {
                        io::stdout().write_all(b"[fixture] warming up\n")?;
                        emit(assistant(prompt))?;
                        emit(result(prompt))?;
                    }
                    "stuck-running" => {
                        emit(
                            json!({"type":"system","subtype":"session_state_changed","state":"running","session_id":"fixture-session"}),
                        )?;
                        emit(result(prompt))?;
                    }
                    "partial" => {
                        let text = serde_json::to_string(&assistant(prompt))?;
                        let midpoint = text.len() / 2;
                        io::stdout().write_all(&text.as_bytes()[..midpoint])?;
                        io::stdout().flush()?;
                        std::thread::sleep(Duration::from_millis(100));
                        io::stdout().write_all(&text.as_bytes()[midpoint..])?;
                        io::stdout().write_all(b"\n")?;
                        io::stdout().flush()?;
                        emit(result(prompt))?;
                    }
                    "stream" => {
                        emit(
                            json!({"type":"stream_event","uuid":"event-id","session_id":"fixture-session","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"streamed"}},"parent_tool_use_id":null}),
                        )?;
                        emit(assistant("streamed"))?;
                        emit(result("streamed"))?;
                    }
                    "full-run" => {
                        emit(
                            json!({"type":"system","subtype":"session_state_changed","state":"running","session_id":"fixture-session"}),
                        )?;
                        emit(result("first-result"))?;
                        std::thread::sleep(Duration::from_millis(80));
                        emit(assistant("background continuation"))?;
                        emit(result("continuation-result"))?;
                        emit(
                            json!({"type":"system","subtype":"session_state_changed","state":"idle","session_id":"fixture-session"}),
                        )?;
                    }
                    "tool-result-null" | "tool-result-values" => {
                        let mut message = json!({"type":"user","session_id":"fixture-session","message":{"role":"user","content":"tool output","future_body":2},"future_outer":1});
                        emit(message.clone())?;
                        let values = if mode == "tool-result-null" {
                            vec![Value::Null]
                        } else {
                            vec![
                                json!(true),
                                json!(42),
                                json!("result"),
                                json!([null, true, {"nested": []}]),
                                json!({"nested": [null, false]}),
                                Value::Null,
                            ]
                        };
                        for value in values {
                            message["tool_use_result"] = value;
                            emit(message.clone())?;
                        }
                        emit(result(prompt))?;
                    }
                    "stream-metadata" => {
                        emit(
                            json!({"type":"stream_event","uuid":"event-id","session_id":"fixture-session","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"streamed"},"future":{"inside":null}},"parent_tool_use_id":null,"ttft_ms":12.5,"user_message_uuid":"user-one","user_message_uuids":["user-one","user-two"],"resume_reason":"future-reason","future":{"outside":[null,true]}}),
                        )?;
                        emit(assistant("streamed"))?;
                        emit(result("streamed"))?;
                    }
                    "metadata" => {
                        emit(
                            json!({"type":"assistant","uuid":"assistant-envelope","session_id":"fixture-session","future_outer":1,"message":{"id":"model-message","model":"fixture-model","content":[{"type":"text","text":"metadata"}],"stop_reason":"end_turn","future_body":2}}),
                        )?;
                        let mut value = result("metadata");
                        value["uuid"] = json!("result-envelope");
                        value["origin"] = json!({"kind":"task-notification","task_id":"task-1"});
                        value["terminal_reason"] = json!("completed");
                        value["modelUsage"] = json!({"fixture-model":{"inputTokens":2}});
                        value["permission_denials"] = json!([{"tool_name":"Bash"}]);
                        value["result_index"] = json!(1);
                        value["future_result"] = json!(3);
                        emit(value)?;
                    }
                    "permission" | "permission-cancel" | "callback-pending" => {
                        emit(
                            json!({"type":"control_request","request_id":"permission-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"printf sample"},"tool_use_id":"tool-id"}}),
                        )?;
                        if mode == "permission-cancel" {
                            emit(json!({"type":"control_cancel_request","request_id":"permission-1"}))?;
                            emit(result("cancelled-permission"))?;
                        }
                    }
                    "hook" => {
                        let callback = initialize["request"]["hooks"]["PreToolUse"][0]["hookCallbackIds"][0]
                            .as_str()
                            .ok_or_else(|| io::Error::other("hook was not initialized"))?;
                        emit(
                            json!({"type":"control_request","request_id":"hook-request","request":{"subtype":"hook_callback","callback_id":callback,"input":{"hook_event_name":"PreToolUse","tool_name":"Bash"},"tool_use_id":"tool-1"}}),
                        )?;
                    }
                    "mcp" | "mcp-retain" => {
                        if mode == "mcp-retain" && input_count > 1 {
                            let response = mcp_step(
                                &mut lines,
                                "mcp-retained-call",
                                json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo","arguments":{"value":"from-fixture"}}}),
                            )?;
                            emit(result(
                                response["result"]["content"][0]["text"]
                                    .as_str()
                                    .unwrap_or("not-initialized"),
                            ))?;
                            continue;
                        }
                        let response = mcp_step(
                            &mut lines,
                            "mcp-init",
                            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}),
                        )?;
                        assert!(response.get("result").is_some());
                        mcp_step(
                            &mut lines,
                            "mcp-notification",
                            json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
                        )?;
                        let response = mcp_step(
                            &mut lines,
                            "mcp-list",
                            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
                        )?;
                        assert_eq!(response["result"]["tools"][0]["name"], "echo");
                        let response = mcp_step(
                            &mut lines,
                            "mcp-call",
                            json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"value":"from-fixture"}}}),
                        )?;
                        emit(result(field(&response["result"]["content"][0], "text")?))?;
                    }
                    "mirror" | "mirror-two" | "mirror-close" => {
                        let config = std::env::var_os("CLAUDE_CONFIG_DIR")
                            .ok_or_else(|| io::Error::other("missing config root"))?;
                        let path =
                            std::path::PathBuf::from(config).join("projects/fixture-project/fixture-session.jsonl");
                        emit(
                            json!({"type":"transcript_mirror","filePath":path,"entries":[{"type":"user","uuid":"entry-1","message":{"content":"one"}}]}),
                        )?;
                        emit(json!({"type":"system","subtype":"mirror_enqueued"}))?;
                        if mode != "mirror-close" {
                            emit(result("mirror-one"))?;
                            emit(
                                json!({"type":"control_request","request_id":"mirror-permission","request":{"subtype":"can_use_tool","tool_name":"Read","input":{}}}),
                            )?;
                            if mode == "mirror-two" {
                                emit(
                                    json!({"type":"transcript_mirror","filePath":path,"entries":[{"type":"assistant","uuid":"entry-2","message":{"content":"two"}}]}),
                                )?;
                                emit(result("mirror-two"))?;
                            }
                        }
                    }
                    "permission-context" | "permission-cli-arg" => emit(
                        json!({"type":"control_request","request_id":"permission-context","request":{"subtype":"can_use_tool","tool_name":"Read","input":{},"tool_use_id":"tool-context","agent_id":"reviewer","permission_suggestions":[{"type":"addRules","rules":[{"toolName":"Read"}],"behavior":"allow","destination":if mode == "permission-cli-arg" { "cliArg" } else { "session" }}],"mcp_server":{"name":"local","source":"sdk"},"title":"Permission title","display_name":"Read file","description":"Permission detail","default_to_no":true,"suppress_always_allow_rule":true,"requires_user_interaction":true}}),
                    )?,
                    "permission-signal" => emit(
                        json!({"type":"control_request","request_id":"permission-1","request":{"subtype":"can_use_tool","tool_name":"Read","input":{}}}),
                    )?,
                    "elicitation" => emit(
                        json!({"type":"control_request","request_id":"elicitation-1","request":{"subtype":"elicitation","mcp_server_name":"local","message":"Input","mode":"form","requested_schema":{"type":"object"}}}),
                    )?,
                    "dialog-undeclared-saturated" | "dialog-undeclared-duplicate" => {
                        emit(
                            json!({"type":"control_request","request_id":"pending-dialog","request":{"subtype":"can_use_tool","tool_name":"Read","input":{}}}),
                        )?;
                        let dialog_id = if mode == "dialog-undeclared-duplicate" {
                            "pending-dialog"
                        } else {
                            "dialog-1"
                        };
                        emit(
                            json!({"type":"control_request","request_id":dialog_id,"request":{"subtype":"request_user_dialog","dialog_kind":"undeclared","payload":{}}}),
                        )?;
                        emit(result("undeclared"))?;
                    }
                    "dialog" | "dialog-undeclared" => {
                        emit(
                            json!({"type":"control_request","request_id":"dialog-1","request":{"subtype":"request_user_dialog","dialog_kind":if mode == "dialog" { "fixture-dialog" } else { "undeclared" },"payload":{"choice":1},"tool_use_id":"tool-dialog"}}),
                        )?;
                        if mode == "dialog-undeclared" {
                            emit(result("undeclared"))?;
                        }
                    }
                    "malformed" => emit(json!({"type":"assistant","message":{"content":[]}}))?,
                    "oversized" => {
                        io::stdout().write_all(&vec![b'x'; 8192])?;
                        io::stdout().write_all(b"\n")?;
                        io::stdout().flush()?;
                    }
                    "failure" => {
                        io::stderr().write_all(&vec![b'Z'; 128 * 1024])?;
                        io::stderr().write_all(b"diagnostic-tail")?;
                        std::process::exit(7);
                    }
                    "error-result-exit" => {
                        emit(
                            json!({"type":"result","subtype":"error_during_execution","is_error":true,"duration_ms":1,"duration_api_ms":1,"num_turns":1,"session_id":"fixture-session","errors":["  boom  ", "", "second"],"api_error_status":529}),
                        )?;
                        std::process::exit(1);
                    }
                    "error-result-success-subtype-exit" => {
                        emit(
                            json!({"type":"result","subtype":"success","is_error":true,"duration_ms":1,"duration_api_ms":1,"num_turns":1,"session_id":"fixture-session","result":"rate limited"}),
                        )?;
                        std::process::exit(1);
                    }
                    "error-result-blank-exit" => {
                        emit(
                            json!({"type":"result","subtype":"error_during_execution","is_error":true,"duration_ms":1,"duration_api_ms":1,"num_turns":1,"session_id":"fixture-session"}),
                        )?;
                        std::process::exit(1);
                    }
                    "corrupt-line" => {
                        io::stdout().write_all(b"{\"type\": oops\n")?;
                        emit(assistant(prompt))?;
                        emit(result(prompt))?;
                    }
                    "corrupt-tail" => {
                        emit(result(prompt))?;
                        io::stdout().write_all(b"{\"type\":\"resu")?;
                        io::stdout().flush()?;
                    }
                    "no-result" => return Ok(()),
                    "unknown-control" => emit(
                        json!({"type":"control_request","request_id":"unknown","request":{"subtype":"future_authority"}}),
                    )?,
                    "queue-overflow" => {
                        for index in 0..100 {
                            emit(json!({"type":"system","subtype":"fixture-event","index":index}))?;
                        }
                    }
                    "continuous" => {
                        let stop = Arc::clone(&stop);
                        flood = Some(std::thread::spawn(move || {
                            let frame = b"{\"type\":\"control_cancel_request\",\"request_id\":\"cancelled\"}\n";
                            let batch = frame.repeat(1024);
                            while !stop.load(Ordering::Relaxed) {
                                if io::stdout().write_all(&batch).is_err() {
                                    break;
                                }
                            }
                        }));
                    }
                    "interrupt" | "control-timeout" => {}
                    "blocked-stdin" => std::thread::sleep(Duration::from_secs(30)),
                    _ => return Err(io::Error::other("unknown fixture mode").into()),
                }
            }
            "control_request" => {
                if mode == "controls" || mode == "mcp" || mode == "mcp-retain" {
                    let request = &value["request"];
                    emit(json!({"type":"system","subtype":"control_observed","request":request}))?;
                    let response = match field(request, "subtype")? {
                        "interrupt" => {
                            if request["cancel_queued"] == true {
                                json!({"still_queued":["queued-one"],"cancelled":["cancelled-one"]})
                            } else {
                                json!({"still_queued":["queued-one"]})
                            }
                        }
                        "rewind_files" => {
                            json!({"canRewind":true,"filesChanged":["README.md"],"insertions":1,"deletions":0})
                        }
                        "mcp_status" => json!({"mcpServers":[{"name":"local","status":"connected"}]}),
                        "get_context_usage" => {
                            json!({"categories":[],"totalTokens":3,"maxTokens":100,"rawMaxTokens":100,"percentage":3,"model":"fixture-model","isAutoCompactEnabled":true,"memoryFiles":[],"mcpTools":[]})
                        }
                        "background_tasks" => json!({"backgrounded":true}),
                        "mcp_set_servers" => {
                            let names = request["servers"]
                                .as_object()
                                .ok_or_else(|| io::Error::other("missing servers"))?
                                .keys()
                                .cloned()
                                .collect::<Vec<_>>();
                            if names.iter().any(|name| name == "reject") {
                                emit(
                                    json!({"type":"control_response","response":{"subtype":"error","request_id":value["request_id"],"error":"fixture rejects configuration"}}),
                                )?;
                                continue;
                            }
                            let removed = configured_servers
                                .iter()
                                .filter(|name| !names.contains(name))
                                .cloned()
                                .collect::<Vec<_>>();
                            let added = names
                                .iter()
                                .filter(|name| !configured_servers.contains(name))
                                .cloned()
                                .collect::<Vec<_>>();
                            configured_servers = names;
                            json!({"added":added,"removed":removed,"errors":{}})
                        }
                        _ => json!({}),
                    };
                    emit(
                        json!({"type":"control_response","response":{"subtype":"success","request_id":value["request_id"],"response":response}}),
                    )?;
                    continue;
                }

                assert_eq!(value["request"]["subtype"], "interrupt");
                if mode == "control-timeout" {
                    continue;
                }
                stop.store(true, Ordering::Relaxed);
                if let Some(flood) = flood.take() {
                    flood
                        .join()
                        .map_err(|_| io::Error::other("fixture flood thread failed"))?;
                }
                acknowledge(field(&value, "request_id")?)?;
                if mode == "permission-signal" {
                    emit(json!({"type":"control_cancel_request","request_id":"permission-1"}))?;
                    emit(result("cancelled"))?;
                }
                if mode == "interrupt" || mode == "continuous" || mode == "callback-pending" {
                    emit(result("interrupted"))?;
                }
            }
            "control_response" => {
                if matches!(
                    mode,
                    "dialog-undeclared" | "dialog-undeclared-saturated" | "dialog-undeclared-duplicate"
                ) {
                    return Err(io::Error::other("undeclared dialog received a response").into());
                }
                if mode == "permission-context" || mode == "permission-cli-arg" {
                    emit(
                        json!({"type":"system","subtype":"permission_observed","response":value["response"]["response"]}),
                    )?;
                    emit(result("context"))?;
                    continue;
                }
                if mode == "elicitation" || mode == "dialog" {
                    emit(result(&serde_json::to_string(&value["response"]["response"])?))?;
                    continue;
                }
                if mode == "hook" {
                    emit(result(&serde_json::to_string(&value["response"]["response"])?))?;
                    continue;
                }
                if mode == "mirror" || mode == "mirror-two" {
                    assert_eq!(value["response"]["request_id"], "mirror-permission");
                    emit(json!({"type":"system","subtype":"mirror_control_answered"}))?;
                    continue;
                }
                if mode == "permission-cancel" {
                    return Err(io::Error::other("cancelled callback sent a response").into());
                }
                if mode == "unknown-control" {
                    assert_eq!(value["response"]["subtype"], "error");
                    emit(result("unsupported-control-rejected"))?;
                } else {
                    assert_eq!(value["response"]["request_id"], "permission-1");
                    assert_eq!(value["response"]["subtype"], "success");
                    let decision = &value["response"]["response"];
                    let behavior = field(decision, "behavior")?;
                    let text = match behavior {
                        "allow" => field(&decision["updatedInput"], "command")?,
                        "deny" => "denied",
                        _ => return Err(io::Error::other("invalid permission behavior").into()),
                    };
                    emit(assistant(text))?;
                    emit(result(text))?;
                }
            }
            _ => return Err(io::Error::other("unexpected SDK frame").into()),
        }
    }
    stop.store(true, Ordering::Relaxed);
    if let Some(flood) = flood.take() {
        flood
            .join()
            .map_err(|_| io::Error::other("fixture flood thread failed"))?;
    }
    Ok(())
}
