//! Wasm runtime errors mapped toward JSON-RPC policy paths (ADR 0024 / 0025).

use std::fmt;

use super::bindings::PolicyDecision;

/// Stable fault codes for `error.data.error` on WASM paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WasmFaultCode {
    Trap,
    FuelExhausted,
    MemoryLimit,
    ImportLimit,
    InputOversize,
    PoolExhausted,
    InitFailed,
    LinkFailed,
    UnknownComponent,
    HostImport,
}

impl WasmFaultCode {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trap => "wasm_trap",
            Self::FuelExhausted => "wasm_fuel_exhausted",
            Self::MemoryLimit => "wasm_memory_limit",
            Self::ImportLimit => "wasm_import_limit",
            Self::InputOversize => "wasm_input_oversize",
            Self::PoolExhausted => "wasm_pool_exhausted",
            Self::InitFailed => "wasm_init_failed",
            Self::LinkFailed => "wasm_link_failed",
            Self::UnknownComponent => "unknown_wasm_component",
            Self::HostImport => "wasm_host_import",
        }
    }
}

/// Outcome of a single guest evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvaluateOutcome {
    Decision(PolicyDecision),
    Fault {
        code: WasmFaultCode,
        message: String,
    },
}

/// Errors surfaced while loading or managing pools.
#[derive(Debug)]
pub enum WasmEngineError {
    Config(String),
    Compile { component_id: String, detail: String },
    Link { component_id: String, detail: String },
    Init { component_id: String, detail: String },
    TargetWit { expected: String, got: String },
    NoComponents,
    UnknownComponent { component_id: String },
    PoolShutDown,
    Internal(String),
}

impl fmt::Display for WasmEngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(detail) => write!(f, "wasm config: {detail}"),
            Self::Compile { component_id, detail } => {
                write!(f, "compile {component_id}: {detail}")
            }
            Self::Link { component_id, detail } => write!(f, "link {component_id}: {detail}"),
            Self::Init { component_id, detail } => write!(f, "init {component_id}: {detail}"),
            Self::TargetWit { expected, got } => {
                write!(f, "target_wit mismatch: expected {expected}, got {got}")
            }
            Self::NoComponents => write!(f, "bundle has no wasm components"),
            Self::UnknownComponent { component_id } => {
                write!(f, "unknown wasm component {component_id}")
            }
            Self::PoolShutDown => write!(f, "component pool shut down"),
            Self::Internal(detail) => write!(f, "wasm internal: {detail}"),
        }
    }
}

impl std::error::Error for WasmEngineError {}
