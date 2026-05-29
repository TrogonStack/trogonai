//! Hand-written Rust bindings for `trogon:mcp-policy@0.1.0` (no `wit-bindgen` yet).
//!
//! Types and traits mirror `wit/trogon-mcp-policy.wit` so item 2 (Wasmtime pooling) can
//! implement `HostImports` and invoke `PolicyGuest` without reshaping the ABI.

/// WIT package name (`manifest.toml` `target_wit` prefix).
pub const WIT_PACKAGE: &str = "trogon:mcp-policy";

/// WIT package semver pin for linker selection.
pub const WIT_VERSION: &str = "0.1.0";

/// Exported world name bundle components must implement.
pub const WORLD_NAME: &str = "policy-bundle";

/// WASI world include pin (vendored snapshot tag).
pub const WASI_CLI_IMPORTS: &str = "wasi:cli/imports@0.3.0-rc-2025-09-16";

/// Host-provided logging levels for `log`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

/// Structured host failure returned from fallible imports and `init`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFailure {
    /// Stable machine code, e.g. `spicedb_unavailable`.
    pub code: String,
    /// Human-readable detail; not a stable contract.
    pub message: String,
}

/// W3C trace context snapshot on each evaluation (ADR 0032).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpanContext {
    /// 32-hex trace id (lowercase, no dashes).
    pub trace_id: String,
    /// Full W3C `traceparent` header value.
    pub traceparent: String,
    /// Optional W3C `tracestate` pass-through.
    pub tracestate: Option<String>,
}

/// MCP tool descriptor for list/catalog shaping paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDescriptor {
    pub name: String,
    pub server_id: String,
    pub description: Option<String>,
    pub input_schema_json: Option<String>,
}

/// Normalized evaluation input — one JSON-RPC request on the gateway path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestCtx {
    pub request_id: String,
    pub actor_id: String,
    pub subject_id: String,
    pub method: String,
    pub params_json: String,
    pub act_chain_json: String,
    pub attributes_json: String,
    pub span: SpanContext,
    pub tools: Vec<ToolDescriptor>,
}

/// Guest decision for a single evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    Allow,
    Deny(String),
    Challenge(String),
}

/// SpiceDB CheckPermission-style triple from `materialize-bindings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpicedbBinding {
    pub object_id: String,
    pub permission: String,
    pub subject: String,
}

/// Host import surface the gateway implements for guests (`interface host`).
pub trait HostImports {
    fn spicedb_check(
        &mut self,
        subject: &str,
        permission: &str,
        object_id: &str,
    ) -> Result<bool, HostFailure>;

    fn cache_get(&mut self, key: &str) -> Option<Vec<u8>>;

    fn cache_set(
        &mut self,
        key: &str,
        value: &[u8],
        ttl_secs: u32,
    ) -> Result<(), HostFailure>;

    fn audit_emit(&mut self, category: &str, fields_json: &str) -> Result<(), HostFailure>;

    fn time_now(&mut self) -> u64;

    fn rate_acquire(
        &mut self,
        scope: &str,
        key: &str,
        budget: u32,
        window_secs: u32,
    ) -> Result<bool, HostFailure>;

    fn jsonpath_read(&mut self, document_json: &str, path: &str) -> Result<String, HostFailure>;

    fn jsonpath_has(&mut self, document_json: &str, path: &str) -> Result<bool, HostFailure>;

    fn log(&mut self, level: LogLevel, message: &str) -> Result<(), HostFailure>;

    fn span_attribute_set(&mut self, key: &str, value: &str) -> Result<(), HostFailure>;

    fn span_event(&mut self, name: &str, attributes_json: &str) -> Result<(), HostFailure>;
}

/// Guest export surface policy components implement (`interface policy-guest`).
pub trait PolicyGuest {
    fn init(&mut self) -> Result<(), HostFailure>;

    fn version(&self) -> String;

    fn evaluate(&mut self, input: &RequestCtx) -> PolicyDecision;

    fn materialize_bindings(
        &mut self,
        input: &RequestCtx,
    ) -> Result<Vec<SpicedbBinding>, HostFailure>;
}

/// Linker metadata for bundle manifest validation and Wasmtime setup (item 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContractIdentity {
    pub package: &'static str,
    pub version: &'static str,
    pub world: &'static str,
    pub wasi_cli_imports: &'static str,
}

/// Stable contract pins mirrored from `wit/trogon-mcp-policy.wit`.
#[must_use]
pub const fn contract_identity() -> ContractIdentity {
    ContractIdentity {
        package: WIT_PACKAGE,
        version: WIT_VERSION,
        world: WORLD_NAME,
        wasi_cli_imports: WASI_CLI_IMPORTS,
    }
}

/// Stub host for tests and compile-time trait checks until Wasmtime links real imports.
#[derive(Debug, Default)]
pub struct UnlinkedHost;

impl HostImports for UnlinkedHost {
    fn spicedb_check(
        &mut self,
        _subject: &str,
        _permission: &str,
        _object_id: &str,
    ) -> Result<bool, HostFailure> {
        Err(HostFailure {
            code: "host_unlinked".into(),
            message: "wasm runtime not wired".into(),
        })
    }

    fn cache_get(&mut self, _key: &str) -> Option<Vec<u8>> {
        None
    }

    fn cache_set(
        &mut self,
        _key: &str,
        _value: &[u8],
        _ttl_secs: u32,
    ) -> Result<(), HostFailure> {
        Err(HostFailure {
            code: "host_unlinked".into(),
            message: "wasm runtime not wired".into(),
        })
    }

    fn audit_emit(&mut self, _category: &str, _fields_json: &str) -> Result<(), HostFailure> {
        Err(HostFailure {
            code: "host_unlinked".into(),
            message: "wasm runtime not wired".into(),
        })
    }

    fn time_now(&mut self) -> u64 {
        0
    }

    fn rate_acquire(
        &mut self,
        _scope: &str,
        _key: &str,
        _budget: u32,
        _window_secs: u32,
    ) -> Result<bool, HostFailure> {
        Err(HostFailure {
            code: "host_unlinked".into(),
            message: "wasm runtime not wired".into(),
        })
    }

    fn jsonpath_read(
        &mut self,
        _document_json: &str,
        _path: &str,
    ) -> Result<String, HostFailure> {
        Err(HostFailure {
            code: "host_unlinked".into(),
            message: "wasm runtime not wired".into(),
        })
    }

    fn jsonpath_has(&mut self, _document_json: &str, _path: &str) -> Result<bool, HostFailure> {
        Err(HostFailure {
            code: "host_unlinked".into(),
            message: "wasm runtime not wired".into(),
        })
    }

    fn log(&mut self, _level: LogLevel, _message: &str) -> Result<(), HostFailure> {
        Err(HostFailure {
            code: "host_unlinked".into(),
            message: "wasm runtime not wired".into(),
        })
    }

    fn span_attribute_set(&mut self, _key: &str, _value: &str) -> Result<(), HostFailure> {
        Err(HostFailure {
            code: "host_unlinked".into(),
            message: "wasm runtime not wired".into(),
        })
    }

    fn span_event(&mut self, _name: &str, _attributes_json: &str) -> Result<(), HostFailure> {
        Err(HostFailure {
            code: "host_unlinked".into(),
            message: "wasm runtime not wired".into(),
        })
    }
}
