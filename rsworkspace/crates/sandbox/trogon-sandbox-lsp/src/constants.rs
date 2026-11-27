use std::time::Duration;

/// How long a request waits for its response unless the caller sets another limit.
pub(crate) const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The JSON-RPC code for a method the client does not implement.
pub(crate) const METHOD_NOT_FOUND: i64 = -32601;

pub(crate) const HEADER_TERMINATOR: &[u8] = b"\r\n\r\n";
pub(crate) const MAX_HEADER_BYTES: usize = 8 * 1024;
pub(crate) const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

pub(crate) const MAX_TRANSPORT_RETRIES: u32 = 3;
pub(crate) const TRANSPORT_RETRY_DELAY: Duration = Duration::from_millis(50);
pub(crate) const MAX_STALLED_RESENDS: u32 = 3;

pub(crate) const EXIT_POLL_MAX_CONSECUTIVE_FAILURES: u32 = 5;
pub(crate) const STOP_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The capabilities the client announces in `initialize`.
pub(crate) const DEFAULT_CLIENT_CAPABILITIES: &str = r#"{
  "workspace": { "configuration": true, "workspaceFolders": true },
  "textDocument": {
    "synchronization": { "didSave": false, "dynamicRegistration": false },
    "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
    "completion": { "completionItem": { "snippetSupport": false, "documentationFormat": ["markdown", "plaintext"] } },
    "hover": { "contentFormat": ["markdown", "plaintext"] },
    "definition": { "linkSupport": true },
    "publishDiagnostics": {}
  }
}"#;

pub(crate) const JSONRPC_VERSION: &str = "2.0";
