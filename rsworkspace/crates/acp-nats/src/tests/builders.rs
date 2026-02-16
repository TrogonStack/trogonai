/// ACP Message Builders - Factory pattern for constructing realistic ACP protocol messages
/// These builders create both protobuf payloads and JSON representations for testing

/// Factory for building realistic ACP InitializeRequest
pub struct AcpInitializeRequestBuilder {
    protocol_version: u32,
    client_name: String,
    client_title: String,
    client_version: String,
    has_fs_read: bool,
    has_fs_write: bool,
    has_terminal: bool,
}

impl AcpInitializeRequestBuilder {
    pub fn new() -> Self {
        Self {
            protocol_version: 1,
            client_name: "zed".to_string(),
            client_title: "Zed".to_string(),
            client_version: "0.220.7".to_string(),
            has_fs_read: true,
            has_fs_write: true,
            has_terminal: true,
        }
    }

    pub fn with_client(mut self, name: &str, title: &str, version: &str) -> Self {
        self.client_name = name.to_string();
        self.client_title = title.to_string();
        self.client_version = version.to_string();
        self
    }

    pub fn with_capabilities(mut self, fs_read: bool, fs_write: bool, terminal: bool) -> Self {
        self.has_fs_read = fs_read;
        self.has_fs_write = fs_write;
        self.has_terminal = terminal;
        self
    }

    pub fn build_payload(&self) -> bytes::Bytes {
        let mut payload = Vec::new();
        payload.push(0x08);
        payload.push(self.protocol_version as u8);
        payload.push(0x12);
        payload.push(self.client_name.len() as u8);
        payload.extend_from_slice(self.client_name.as_bytes());
        let mut caps = 0u8;
        if self.has_fs_read {
            caps |= 0x01;
        }
        if self.has_fs_write {
            caps |= 0x02;
        }
        if self.has_terminal {
            caps |= 0x04;
        }
        payload.push(0x18);
        payload.push(caps);
        bytes::Bytes::from(payload)
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "outgoing",
            "_type": "request",
            "id": 0,
            "method": "initialize",
            "params": {
                "protocolVersion": self.protocol_version,
                "clientCapabilities": {
                    "fs": {
                        "readTextFile": self.has_fs_read,
                        "writeTextFile": self.has_fs_write
                    },
                    "terminal": self.has_terminal,
                    "_meta": {
                        "terminal_output": true,
                        "terminal-auth": true
                    }
                },
                "clientInfo": {
                    "name": self.client_name,
                    "title": self.client_title,
                    "version": self.client_version
                }
            }
        })
    }
}

/// Factory for building realistic ACP SessionNew request
pub struct AcpSessionNewRequestBuilder {
    cwd: String,
    mcp_servers: Vec<serde_json::Value>,
}

impl AcpSessionNewRequestBuilder {
    pub fn new(cwd: &str) -> Self {
        Self {
            cwd: cwd.to_string(),
            mcp_servers: vec![],
        }
    }

    pub fn with_mcp_server(mut self, name: &str, command: &str) -> Self {
        self.mcp_servers.push(serde_json::json!({
            "name": name,
            "command": command
        }));
        self
    }

    pub fn with_mcp_server_full(mut self, server: serde_json::Value) -> Self {
        self.mcp_servers.push(server);
        self
    }

    pub fn build_payload(&self) -> bytes::Bytes {
        let mut payload = Vec::new();
        payload.push(0x0a);
        payload.push(self.cwd.len() as u8);
        payload.extend_from_slice(self.cwd.as_bytes());
        payload.push(0x10);
        payload.push(self.mcp_servers.len() as u8);
        bytes::Bytes::from(payload)
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "outgoing",
            "_type": "request",
            "id": 1,
            "method": "session/new",
            "params": {
                "cwd": self.cwd,
                "mcpServers": self.mcp_servers
            }
        })
    }
}

/// Factory for building realistic ACP SessionNew response
pub struct AcpSessionNewResponseBuilder {
    session_id: String,
}

impl AcpSessionNewResponseBuilder {
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
        }
    }

    pub fn build_payload(&self) -> bytes::Bytes {
        let mut payload = Vec::new();
        payload.push(0x0a);
        payload.push(self.session_id.len() as u8);
        payload.extend_from_slice(self.session_id.as_bytes());
        payload.push(0x10);
        payload.push(0x05);
        bytes::Bytes::from(payload)
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "incoming",
            "_type": "response",
            "id": 1,
            "method": "session/new",
            "params": {
                "sessionId": self.session_id,
                "modes": {
                    "currentModeId": "chat",
                    "availableModes": [
                        {"id": "chat", "name": "Chat", "description": "Conversational assistant"},
                        {"id": "insert", "name": "Insert", "description": "Direct content insertion"},
                        {"id": "edit", "name": "Edit", "description": "Modify existing code"},
                        {"id": "command", "name": "Command", "description": "Execute tasks"},
                        {"id": "review", "name": "Review", "description": "Analyze code"}
                    ]
                }
            }
        })
    }
}

/// Factory for building session update notification
pub struct AcpSessionUpdateNotificationBuilder {
    session_id: String,
    command_count: usize,
}

impl AcpSessionUpdateNotificationBuilder {
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            command_count: 8,
        }
    }

    pub fn with_commands(mut self, count: usize) -> Self {
        self.command_count = count;
        self
    }

    pub fn build_payload(&self) -> bytes::Bytes {
        let mut payload = Vec::new();
        payload.push(0x0a);
        payload.push(self.session_id.len() as u8);
        payload.extend_from_slice(self.session_id.as_bytes());
        payload.push(0x10);
        payload.push(self.command_count as u8);
        bytes::Bytes::from(payload)
    }

    pub fn build_json(&self) -> serde_json::Value {
        let commands = vec![
            ("test", "Run tests for the current project"),
            ("plan", "Create a detailed implementation plan"),
            ("review", "Review code and provide suggestions"),
            ("explain", "Explain code or concepts in detail"),
            ("fix", "Find and fix issues in code"),
            ("refactor", "Refactor code to improve quality"),
            ("doc", "Generate documentation for code"),
            ("hello", "[HARDCODED TEST] Say hello to someone"),
        ];

        serde_json::json!({
            "_direction": "incoming",
            "_type": "notification",
            "id": null,
            "method": "session/update",
            "params": {
                "sessionId": self.session_id,
                "update": {
                    "sessionUpdate": "available_commands_update",
                    "availableCommands": commands[0..self.command_count].iter().map(|(name, desc)| {
                        serde_json::json!({
                            "name": name,
                            "description": desc,
                            "input": null
                        })
                    }).collect::<Vec<_>>()
                }
            }
        })
    }
}

/// Factory for building session prompt request
pub struct AcpSessionPromptRequestBuilder {
    session_id: String,
    prompt_text: String,
}

impl AcpSessionPromptRequestBuilder {
    pub fn new(session_id: &str, prompt_text: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            prompt_text: prompt_text.to_string(),
        }
    }

    pub fn build_payload(&self) -> bytes::Bytes {
        let mut payload = Vec::new();
        payload.push(0x0a);
        payload.push(self.session_id.len() as u8);
        payload.extend_from_slice(self.session_id.as_bytes());
        payload.push(0x12);
        payload.push(self.prompt_text.len() as u8);
        payload.extend_from_slice(self.prompt_text.as_bytes());
        bytes::Bytes::from(payload)
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "outgoing",
            "_type": "request",
            "id": 2,
            "method": "session/prompt",
            "params": {
                "sessionId": self.session_id,
                "prompt": [
                    {
                        "type": "text",
                        "text": self.prompt_text
                    }
                ]
            }
        })
    }
}

/// Factory for building session cancel notification
pub struct AcpSessionCancelNotificationBuilder {
    session_id: String,
}

impl AcpSessionCancelNotificationBuilder {
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
        }
    }

    pub fn build_payload(&self) -> bytes::Bytes {
        let mut payload = Vec::new();
        payload.push(0x0a);
        payload.push(self.session_id.len() as u8);
        payload.extend_from_slice(self.session_id.as_bytes());
        bytes::Bytes::from(payload)
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "outgoing",
            "_type": "notification",
            "id": null,
            "method": "session/cancel",
            "params": {
                "sessionId": self.session_id
            }
        })
    }
}

/// Factory for building agent message chunk notification (streaming)
pub struct AcpAgentMessageChunkBuilder {
    session_id: String,
    chunk_text: String,
}

impl AcpAgentMessageChunkBuilder {
    pub fn new(session_id: &str, chunk_text: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            chunk_text: chunk_text.to_string(),
        }
    }

    pub fn build_payload(&self) -> bytes::Bytes {
        let mut payload = Vec::new();
        payload.push(0x0a);
        payload.push(self.session_id.len() as u8);
        payload.extend_from_slice(self.session_id.as_bytes());
        payload.push(0x12);
        payload.push(self.chunk_text.len() as u8);
        payload.extend_from_slice(self.chunk_text.as_bytes());
        bytes::Bytes::from(payload)
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "incoming",
            "_type": "notification",
            "id": null,
            "method": "session/update",
            "params": {
                "sessionId": self.session_id,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "content": {
                        "type": "text",
                        "text": self.chunk_text
                    }
                }
            }
        })
    }
}

/// Factory for building prompt response (end of streaming)
pub struct AcpSessionPromptResponseBuilder {
    stop_reason: String,
    request_id: i32,
}

impl AcpSessionPromptResponseBuilder {
    pub fn new(stop_reason: &str) -> Self {
        Self {
            stop_reason: stop_reason.to_string(),
            request_id: 2,
        }
    }

    pub fn with_id(mut self, request_id: i32) -> Self {
        self.request_id = request_id;
        self
    }

    pub fn build_payload(&self) -> bytes::Bytes {
        let mut payload = Vec::new();
        payload.push(0x0a);
        payload.push(self.stop_reason.len() as u8);
        payload.extend_from_slice(self.stop_reason.as_bytes());
        bytes::Bytes::from(payload)
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "incoming",
            "_type": "response",
            "id": self.request_id,
            "method": "session/prompt",
            "params": {
                "stopReason": self.stop_reason
            }
        })
    }
}

/// Factory for building agent thought chunk
pub struct AcpAgentThoughtChunkBuilder {
    session_id: String,
    thought_text: String,
}

impl AcpAgentThoughtChunkBuilder {
    pub fn new(session_id: &str, thought_text: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            thought_text: thought_text.to_string(),
        }
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "incoming",
            "_type": "notification",
            "id": null,
            "method": "session/update",
            "params": {
                "sessionId": self.session_id,
                "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "content": {
                        "type": "text",
                        "text": self.thought_text
                    }
                }
            }
        })
    }
}

/// Factory for building tool call notification
pub struct AcpToolCallBuilder {
    session_id: String,
    tool_call_id: String,
    title: String,
    command: String,
    args: Vec<String>,
    cwd: String,
    kind: String,
    entries: Vec<serde_json::Value>,
}

impl AcpToolCallBuilder {
    pub fn new(session_id: &str, tool_call_id: &str, command: &str, cwd: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            tool_call_id: tool_call_id.to_string(),
            title: format!("Executing: {}", command),
            command: command.to_string(),
            args: vec!["+%Y-%m-%d %H:%M:%S".to_string()],
            cwd: cwd.to_string(),
            kind: "execute".to_string(),
            entries: vec![],
        }
    }

    pub fn with_args(mut self, args: Vec<&str>) -> Self {
        self.args = args.iter().map(|s| s.to_string()).collect();
        self
    }

    pub fn with_kind(mut self, kind: &str) -> Self {
        self.kind = kind.to_string();
        self
    }

    pub fn with_entry(mut self, content: &str, priority: &str, status: &str) -> Self {
        self.entries.push(serde_json::json!({
            "content": content,
            "priority": priority,
            "status": status
        }));
        self
    }

    pub fn build_json(&self) -> serde_json::Value {
        let mut raw_input = serde_json::json!({
            "args": self.args,
            "command": self.command,
            "cwd": self.cwd
        });

        if !self.entries.is_empty() {
            raw_input["entries"] = serde_json::Value::Array(self.entries.clone());
        }

        serde_json::json!({
            "_direction": "incoming",
            "_type": "notification",
            "id": null,
            "method": "session/update",
            "params": {
                "sessionId": self.session_id,
                "update": {
                    "sessionUpdate": "tool_call",
                    "toolCallId": self.tool_call_id,
                    "title": self.title,
                    "kind": self.kind,
                    "status": "in_progress",
                    "rawInput": raw_input
                }
            }
        })
    }
}

/// Factory for building tool call update (result)
pub struct AcpToolCallUpdateBuilder {
    session_id: String,
    tool_call_id: String,
    output: String,
    exit_code: i32,
}

impl AcpToolCallUpdateBuilder {
    pub fn new(session_id: &str, tool_call_id: &str, output: &str, exit_code: i32) -> Self {
        Self {
            session_id: session_id.to_string(),
            tool_call_id: tool_call_id.to_string(),
            output: output.to_string(),
            exit_code,
        }
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "incoming",
            "_type": "notification",
            "id": null,
            "method": "session/update",
            "params": {
                "sessionId": self.session_id,
                "update": {
                    "sessionUpdate": "tool_call_update",
                    "toolCallId": self.tool_call_id,
                    "status": "completed",
                    "content": [
                        {
                            "type": "content",
                            "content": {
                                "type": "text",
                                "text": self.output
                            }
                        }
                    ],
                    "rawOutput": {
                        "exitCode": self.exit_code,
                        "output": self.output,
                        "truncated": false
                    }
                }
            }
        })
    }
}

/// Factory for building plan update notification
pub struct AcpPlanUpdateBuilder {
    session_id: String,
    entries: Vec<serde_json::Value>,
}

impl AcpPlanUpdateBuilder {
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            entries: vec![],
        }
    }

    pub fn with_entry(mut self, title: &str, description: &str) -> Self {
        self.entries.push(serde_json::json!({
            "title": title,
            "description": description
        }));
        self
    }

    pub fn with_entry_detailed(mut self, content: &str, priority: &str, status: &str) -> Self {
        self.entries.push(serde_json::json!({
            "content": content,
            "priority": priority,
            "status": status
        }));
        self
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "incoming",
            "_type": "notification",
            "id": null,
            "method": "session/update",
            "params": {
                "sessionId": self.session_id,
                "update": {
                    "sessionUpdate": "plan",
                    "entries": self.entries
                }
            }
        })
    }
}

/// Factory for building available commands update notification
pub struct AcpAvailableCommandsUpdateBuilder {
    session_id: String,
    commands: Vec<serde_json::Value>,
}

impl AcpAvailableCommandsUpdateBuilder {
    pub fn new(session_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            commands: vec![],
        }
    }

    pub fn with_command(mut self, name: &str, description: &str, input_hint: Option<&str>) -> Self {
        let input = if let Some(hint) = input_hint {
            serde_json::json!({ "hint": hint })
        } else {
            serde_json::Value::Null
        };

        self.commands.push(serde_json::json!({
            "name": name,
            "description": description,
            "input": input
        }));
        self
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "incoming",
            "_type": "notification",
            "id": null,
            "method": "session/update",
            "params": {
                "sessionId": self.session_id,
                "update": {
                    "sessionUpdate": "available_commands_update",
                    "availableCommands": self.commands
                }
            }
        })
    }
}

/// Factory for building MCP server configuration
pub struct AcpMcpServerBuilder {
    name: String,
    command: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
}

impl AcpMcpServerBuilder {
    pub fn new(name: &str, command: &str) -> Self {
        Self {
            name: name.to_string(),
            command: command.to_string(),
            args: vec![],
            env: vec![],
        }
    }

    pub fn with_arg(mut self, arg: &str) -> Self {
        self.args.push(arg.to_string());
        self
    }

    pub fn with_env(mut self, key: &str, value: &str) -> Self {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    pub fn build_json(&self) -> serde_json::Value {
        let env_array: Vec<_> = self
            .env
            .iter()
            .map(|(k, v)| serde_json::json!({ "name": k, "value": v }))
            .collect();

        serde_json::json!({
            "name": self.name,
            "command": self.command,
            "args": self.args,
            "env": env_array
        })
    }
}

/// Factory for terminal/create request
pub struct AcpTerminalCreateRequestBuilder {
    session_id: String,
    command: String,
    args: Vec<String>,
    cwd: String,
}

impl AcpTerminalCreateRequestBuilder {
    pub fn new(session_id: &str, command: &str, cwd: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            command: command.to_string(),
            args: vec!["+%Y-%m-%d %H:%M:%S".to_string()],
            cwd: cwd.to_string(),
        }
    }

    pub fn with_args(mut self, args: Vec<&str>) -> Self {
        self.args = args.iter().map(|s| s.to_string()).collect();
        self
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "incoming",
            "_type": "request",
            "id": 0,
            "method": "terminal/create",
            "params": {
                "sessionId": self.session_id,
                "command": self.command,
                "args": self.args,
                "cwd": self.cwd,
                "outputByteLimit": 100000
            }
        })
    }
}

/// Factory for terminal/create response
pub struct AcpTerminalCreateResponseBuilder {
    terminal_id: String,
}

impl AcpTerminalCreateResponseBuilder {
    pub fn new(terminal_id: &str) -> Self {
        Self {
            terminal_id: terminal_id.to_string(),
        }
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "outgoing",
            "_type": "response",
            "id": 0,
            "method": "terminal/create",
            "params": {
                "terminalId": self.terminal_id
            }
        })
    }
}

/// Factory for terminal/wait_for_exit request
pub struct AcpTerminalWaitForExitRequestBuilder {
    session_id: String,
    terminal_id: String,
}

impl AcpTerminalWaitForExitRequestBuilder {
    pub fn new(session_id: &str, terminal_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            terminal_id: terminal_id.to_string(),
        }
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "incoming",
            "_type": "request",
            "id": 1,
            "method": "terminal/wait_for_exit",
            "params": {
                "sessionId": self.session_id,
                "terminalId": self.terminal_id
            }
        })
    }
}

/// Factory for terminal/wait_for_exit response
pub struct AcpTerminalWaitForExitResponseBuilder {
    exit_code: i32,
}

impl AcpTerminalWaitForExitResponseBuilder {
    pub fn new(exit_code: i32) -> Self {
        Self { exit_code }
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "outgoing",
            "_type": "response",
            "id": 1,
            "method": "terminal/wait_for_exit",
            "params": {
                "exitCode": self.exit_code,
                "signal": null
            }
        })
    }
}

/// Factory for terminal/output request
pub struct AcpTerminalOutputRequestBuilder {
    session_id: String,
    terminal_id: String,
}

impl AcpTerminalOutputRequestBuilder {
    pub fn new(session_id: &str, terminal_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            terminal_id: terminal_id.to_string(),
        }
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "incoming",
            "_type": "request",
            "id": 2,
            "method": "terminal/output",
            "params": {
                "sessionId": self.session_id,
                "terminalId": self.terminal_id
            }
        })
    }
}

/// Factory for terminal/output response
pub struct AcpTerminalOutputResponseBuilder {
    output: String,
    exit_code: i32,
}

impl AcpTerminalOutputResponseBuilder {
    pub fn new(output: &str, exit_code: i32) -> Self {
        Self {
            output: output.to_string(),
            exit_code,
        }
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "outgoing",
            "_type": "response",
            "id": 2,
            "method": "terminal/output",
            "params": {
                "output": self.output,
                "truncated": false,
                "exitStatus": {
                    "exitCode": self.exit_code,
                    "signal": null
                }
            }
        })
    }
}

/// Factory for terminal/release request
pub struct AcpTerminalReleaseRequestBuilder {
    session_id: String,
    terminal_id: String,
}

impl AcpTerminalReleaseRequestBuilder {
    pub fn new(session_id: &str, terminal_id: &str) -> Self {
        Self {
            session_id: session_id.to_string(),
            terminal_id: terminal_id.to_string(),
        }
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "incoming",
            "_type": "request",
            "id": 3,
            "method": "terminal/release",
            "params": {
                "sessionId": self.session_id,
                "terminalId": self.terminal_id
            }
        })
    }
}

/// Factory for terminal/release response
pub struct AcpTerminalReleaseResponseBuilder;

impl AcpTerminalReleaseResponseBuilder {
    pub fn new() -> Self {
        Self
    }

    pub fn build_json(&self) -> serde_json::Value {
        serde_json::json!({
            "_direction": "outgoing",
            "_type": "response",
            "id": 3,
            "method": "terminal/release",
            "params": {}
        })
    }
}

/// Factory for building realistic ACP InitializeResponse
pub struct AcpInitializeResponseBuilder {
    success: bool,
    error_code: Option<i32>,
    error_message: Option<String>,
}

impl AcpInitializeResponseBuilder {
    pub fn success() -> Self {
        Self {
            success: true,
            error_code: None,
            error_message: None,
        }
    }

    pub fn error(code: i32, message: &str) -> Self {
        Self {
            success: false,
            error_code: Some(code),
            error_message: Some(message.to_string()),
        }
    }

    pub fn build_payload(&self) -> bytes::Bytes {
        let mut payload = Vec::new();

        if self.success {
            payload.push(0x0a);
            payload.push(0x00);
        } else if let (Some(code), Some(msg)) = (self.error_code, self.error_message.as_ref()) {
            payload.push(0x08);
            payload.extend_from_slice(&(code as u32).to_le_bytes()[0..1]);
            payload.push(0x12);
            payload.push(msg.len() as u8);
            payload.extend_from_slice(msg.as_bytes());
        }

        bytes::Bytes::from(payload)
    }

    pub fn build_json(&self) -> serde_json::Value {
        if self.success {
            serde_json::json!({
                "_direction": "incoming",
                "_type": "response",
                "id": 0,
                "method": "initialize",
                "params": {
                    "serverCapabilities": {
                        "prompts": true,
                        "resources": true,
                        "tools": true
                    }
                }
            })
        } else {
            let code = self.error_code.unwrap_or(-32603);
            let msg = self
                .error_message
                .as_ref()
                .map(|m| m.as_str())
                .unwrap_or("Unknown error");
            serde_json::json!({
                "_direction": "incoming",
                "_type": "response",
                "id": 0,
                "method": "initialize",
                "params": {
                    "code": code,
                    "message": msg
                }
            })
        }
    }
}

/// Helper function for structured step output
#[allow(dead_code)]
pub fn step(_number: usize, _title: &str) {}

/// Helper function for step completion
#[allow(dead_code)]
pub fn step_complete() {}
