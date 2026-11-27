#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

//! Worker that fulfills sandbox lease requests by running untrusted JS in a V8 isolate or
//! WASM components in Wasmtime, with no hardware virtualization required.

mod build_heartbeat;
mod cancel_dispatch;
mod cancel_lease_id;
mod captured_output;
mod constants;
mod dispatch_failure;
mod guest_kind;
mod guest_outcome;
mod js_guest;
mod js_runtime;
mod js_runtime_error;
mod js_session;
mod lease_dispatch;
mod nats_lifecycle_emitter;
mod persistent_runtime;
mod profile;
mod provider_state;
mod publish_reply;
mod retire_error;
mod retire_outcome;
mod routed_reply;
mod routed_verb;
mod routing;
mod run_lease;
mod run_worker;
mod serve;
mod serve_error;
mod session_command;
mod session_command_error;
mod v8_lease;
mod v8_wasm_authority_host;
mod v8_wasm_lease_error;
mod v8_wasm_provider;
mod wasm_runtime;
mod wasm_runtime_error;
mod worker_args;
mod worker_binding_error;
mod worker_error;
mod worker_event;

pub use build_heartbeat::build_heartbeat;
pub use cancel_lease_id::extract_cancel_lease_id;
pub use captured_output::CapturedOutput;
pub use constants::{EVAL_COMMAND, LEASE_QUEUE_GROUP, MAX_PERSISTENT_RUNTIMES, PERSISTENT_RUNTIME_ENV};
pub use guest_outcome::GuestOutcome;
pub use js_runtime::run_js;
pub use js_runtime_error::JsRuntimeError;
pub use js_session::JsSession;
pub use nats_lifecycle_emitter::NatsLifecycleEmitter;
pub use profile::PROFILE;
pub use retire_error::RetireError;
pub use routed_verb::RoutedVerb;
pub use run_worker::run_worker;
pub use serve::serve;
pub use serve_error::ServeError;
pub use v8_wasm_lease_error::V8WasmLeaseError;
pub use v8_wasm_provider::V8WasmProvider;
pub use wasm_runtime::run_wasm;
pub use wasm_runtime_error::WasmRuntimeError;
pub use worker_args::WorkerArgs;
pub use worker_binding_error::WorkerBindingError;
pub use worker_error::WorkerError;
