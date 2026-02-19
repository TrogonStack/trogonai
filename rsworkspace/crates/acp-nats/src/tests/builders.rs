//! ACP Message Builders - Factory pattern for constructing realistic ACP protocol messages
//! These builders create both protobuf payloads and JSON representations for testing

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

    pub fn build_payload(&self) -> bytes::Bytes {
        let mut payload = vec![
            0x08,
            self.protocol_version as u8,
            0x12,
            self.client_name.len() as u8,
        ];
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
            let msg = self.error_message.as_deref().unwrap_or("Unknown error");
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
