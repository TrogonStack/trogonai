use crate::config::Config;
use crate::metrics::METRICS;
use crate::traits::{Clock, NatsBroker, StdClock, StdSyncFs, SyncFs, WasmExecutor, WasmRunConfig};
use agent_client_protocol::TerminalExitStatus;
use futures::StreamExt as _;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncReadExt;
use wasmtime::{
    Caller, Engine, Extern, InstancePre, Linker, Module, ResourceLimiter, Store, ValType,
};
use wasmtime_wasi::pipe::AsyncWriteStream;
use wasmtime_wasi::preview1::{self, WasiP1Ctx};
use wasmtime_wasi::{AsyncStdoutStream, DirPerms, FilePerms, WasiCtxBuilder};

// ── Execution config ─────────────────────────────────────────────────────────

/// All parameters needed to execute one WASM module invocation.
/// Replaces the previous 17-argument function signature.
pub(crate) struct WasmExecConfig<N: NatsBroker + Send + Sync = async_nats::Client> {
    /// Pre-linked instance ready for `instantiate_async`.
    /// Built once per unique module path and cached in `WasmRuntime`.
    pub instance_pre: InstancePre<WasmStoreData<N>>,
    /// Full argv, including argv[0] (the module path / program name).
    pub argv: Vec<String>,
    /// Environment variables to expose inside the module.
    pub env_vars: Vec<(String, String)>,
    /// Host directory preopened as `/` (the session sandbox root).
    pub sandbox_dir: PathBuf,
    /// Optional working directory preopened as `.` when different from `sandbox_dir`.
    pub cwd: Option<PathBuf>,
    /// Shared buffer where stdout/stderr are appended.
    pub output_buf: Arc<Mutex<Vec<u8>>>,
    /// Set to `true` by the I/O reader tasks when bytes are dropped due to the limit.
    pub was_truncated: Arc<std::sync::atomic::AtomicBool>,
    /// Maximum bytes kept in `output_buf`; oldest bytes are dropped when exceeded.
    pub output_byte_limit: usize,
    /// Optional wall-clock execution timeout.
    pub timeout_secs: Option<u64>,
    /// Optional linear-memory limit in bytes.
    pub memory_limit_bytes: Option<usize>,
    /// Optional NATS client for trogon.* host functions.
    pub nats_client: Option<N>,
    /// Session identifier forwarded to host functions for logging / NATS subjects.
    pub session_id: String,
    /// Allow WASI network access (`inherit_network`).
    pub allow_network: bool,
    /// Auto-select the first permission option instead of asking over NATS.
    pub auto_allow_permissions: bool,
    /// Wasmtime fuel budget (instruction limit). 0 means u64::MAX.
    pub fuel_limit: u64,
    /// Maximum number of trogon.* host calls. Returns -1 when exhausted.
    pub host_call_limit: u32,
    /// ACP subject prefix for NATS permission requests.
    pub acp_prefix: String,
}

// ── Store state ─────────────────────────────────────────────────────────────

/// Custom store state that wraps WASI context plus limits and host state.
pub(crate) struct WasmStoreData<N: NatsBroker + Send + Sync = async_nats::Client> {
    pub wasi: WasiP1Ctx,
    pub limits: StoreLimitsData,
    pub nats: Option<N>,
    pub session_id: String,
    /// When `true`, `request_permission` auto-selects the first option.
    pub auto_allow_permissions: bool,
    /// Remaining host-call budget. Decremented on each trogon.* call.
    /// When zero, host functions return -1 without executing.
    pub host_call_budget: u32,
    /// Active NATS subscriptions keyed by subscription ID.
    pub subscriptions: HashMap<i32, N::Sub>,
    /// Counter for next subscription ID.
    pub next_sub_id: i32,
    /// ACP subject prefix for permission requests over NATS.
    pub acp_prefix: String,
}

/// Memory limit state threaded through the store.
pub(crate) struct StoreLimitsData {
    pub memory_limit: Option<usize>,
}

impl<N: NatsBroker + Send + Sync> ResourceLimiter for WasmStoreData<N> {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        if let Some(limit) = self.limits.memory_limit {
            if desired > limit {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        Ok(true)
    }
}

// ── Memory helpers ───────────────────────────────────────────────────────────

fn read_str(mem: &[u8], ptr: usize, len: usize) -> Option<&str> {
    let bytes = mem.get(ptr..ptr.checked_add(len)?)?;
    std::str::from_utf8(bytes).ok()
}

fn read_bytes(mem: &[u8], ptr: usize, len: usize) -> Option<&[u8]> {
    mem.get(ptr..ptr.checked_add(len)?)
}

// ── Host function registration ───────────────────────────────────────────────
//
// trogon_v1 Host ABI — stable signatures (breaking changes require "trogon_v2")
//
// WASM modules import these functions from the "trogon_v1" module. Any change
// to parameter count, types, or semantics is a BREAKING CHANGE that must be
// deployed as a new import module name (trogon_v2, trogon_v3, …) so that old
// modules get a link error instead of a silent trap.
//
// Current stable interface:
//   log(level_ptr i32, level_len i32, msg_ptr i32, msg_len i32)
//     NOTE: the level string is forwarded as a structured tracing field but
//     the host always emits the event at INFO level. This is intentional —
//     WASM modules must not be able to inject false ERROR/WARN events into
//     the host's observability pipeline and trigger spurious alerts.
//   nats_publish(subj_ptr i32, subj_len i32, payload_ptr i32, payload_len i32) -> i32
//   nats_request(subj_ptr i32, subj_len i32, pay_ptr i32, pay_len i32,
//                timeout_ms i32, out_ptr i32, out_max i32) -> i32
//   subscribe(subject_ptr i32, subject_len i32) -> i32
//   recv_message(sub_id i32,
//                out_subj_ptr i32, out_subj_max i32, out_subj_len_ptr i32,
//                out_payload_ptr i32, out_payload_max i32, out_payload_len_ptr i32,
//                timeout_ms i32) -> i32
//   unsubscribe(sub_id i32) -> i32
//   request_permission(options_json_ptr i32, options_json_len i32,
//                      out_selected_ptr i32) -> i32
//     NOTE: when auto_allow_permissions=false, this publishes a NATS request to
//     `{acp_prefix}.session.{session_id}.wasm.request_permission` using the
//     ACP-standard RequestPermissionRequest schema (sessionId, toolCall, options
//     as [{optionId, title}]) and expects a RequestPermissionOutcome reply:
//     `{"outcome":"selected","optionId":"<index>"}` or `{"outcome":"cancelled"}`.
//     The optionId in the request uses the string representation of the array
//     index so the host can map it back to the WASM ABI's integer output.
//     This subject is NOT the standard ACP session.request_permission method —
//     it is a custom trogon extension that requires a dedicated handler on the
//     client side (the standard method is client→agent, not agent→runtime).
//
// Lifecycle notes:
//   - WASM module kill is implemented via task abort() — the module has no
//     opportunity for cleanup (WASM has no signal handlers). This is intentional
//     for kill semantics.
//   - Sessions are not persisted across runtime restarts. cleanup_stale_sessions()
//     deletes all sandbox directories at startup by design (clean slate).

/// Maximum number of simultaneous active NATS subscriptions a single WASM module
/// execution may hold. Prevents a runaway module from exhausting the shared NATS
/// connection with thousands of idle subscribers. The host_call_limit provides
/// a secondary bound on total calls but not on concurrent live subscribers.
const MAX_SUBSCRIPTIONS_PER_MODULE: usize = 64;

/// Builds a `Linker` with all WASI (WASIp1) and `trogon_v1` host functions registered.
///
/// Called once per `WasmRuntime` instance; the resulting linker is stored on the runtime
/// and reused across all module invocations. Call `linker.instantiate_pre(&module)` to
/// produce an `InstancePre` that can be cached per-module and cloned per invocation.
pub(crate) fn build_linker<N: NatsBroker + Send + Sync + 'static>(
    engine: &Engine,
) -> anyhow::Result<Linker<WasmStoreData<N>>> {
    let mut linker: Linker<WasmStoreData<N>> = Linker::new(engine);
    preview1::add_to_linker_async(&mut linker, |t| &mut t.wasi)?;
    add_trogon_host_functions::<N>(engine, &mut linker)?;
    Ok(linker)
}

fn add_trogon_host_functions<N: NatsBroker + Send + Sync + 'static>(
    engine: &Engine,
    linker: &mut Linker<WasmStoreData<N>>,
) -> anyhow::Result<()> {
    // trogon_v1.log(level_ptr, level_len, msg_ptr, msg_len)
    linker.func_new_async(
        "trogon_v1",
        "log",
        wasmtime::FuncType::new(
            engine,
            [ValType::I32, ValType::I32, ValType::I32, ValType::I32],
            [],
        ),
        |mut caller: Caller<'_, WasmStoreData<N>>, params, _results| {
            let level_ptr = params[0].unwrap_i32() as usize;
            let level_len = params[1].unwrap_i32() as usize;
            let msg_ptr = params[2].unwrap_i32() as usize;
            let msg_len = params[3].unwrap_i32() as usize;
            Box::new(async move {
                // Check and decrement host call budget; return -1 if exhausted.
                if caller.data().host_call_budget == 0 {
                    return Ok(());
                }
                caller.data_mut().host_call_budget -= 1;
                METRICS
                    .host_calls_total
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                let mem = match caller.get_export("memory").and_then(|e| {
                    if let Extern::Memory(m) = e {
                        Some(m)
                    } else {
                        None
                    }
                }) {
                    Some(m) => m,
                    None => return Ok(()),
                };
                let data = mem.data(&caller);
                let level = read_str(data, level_ptr, level_len)
                    .unwrap_or("info")
                    .to_owned();
                let msg = read_str(data, msg_ptr, msg_len)
                    .unwrap_or("(invalid utf8)")
                    .to_owned();
                let session_id = caller.data().session_id.clone();
                let _ = data; // release borrow before logging
                tracing::info!(
                    wasm_log_level = %level,
                    session_id = %session_id,
                    "{}",
                    msg
                );
                Ok(())
            })
        },
    )?;

    // trogon_v1.nats_publish(subj_ptr, subj_len, payload_ptr, payload_len) -> i32
    linker.func_new_async(
        "trogon_v1",
        "nats_publish",
        wasmtime::FuncType::new(
            engine,
            [ValType::I32, ValType::I32, ValType::I32, ValType::I32],
            [ValType::I32],
        ),
        |mut caller: Caller<'_, WasmStoreData<N>>, params, results| {
            let subj_ptr = params[0].unwrap_i32() as usize;
            let subj_len = params[1].unwrap_i32() as usize;
            let payload_ptr = params[2].unwrap_i32() as usize;
            let payload_len = params[3].unwrap_i32() as usize;
            Box::new(async move {
                // Check and decrement host call budget; return -1 if exhausted.
                if caller.data().host_call_budget == 0 {
                    results[0] = wasmtime::Val::I32(-1);
                    return Ok(());
                }
                caller.data_mut().host_call_budget -= 1;
                METRICS
                    .host_calls_total
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                let mem = match caller.get_export("memory").and_then(|e| {
                    if let Extern::Memory(m) = e {
                        Some(m)
                    } else {
                        None
                    }
                }) {
                    Some(m) => m,
                    None => {
                        results[0] = wasmtime::Val::I32(-1);
                        return Ok(());
                    }
                };
                let data = mem.data(&caller);
                let subject = match read_str(data, subj_ptr, subj_len) {
                    Some(s) => s.to_owned(),
                    None => {
                        results[0] = wasmtime::Val::I32(-1);
                        return Ok(());
                    }
                };
                let payload_bytes = match read_bytes(data, payload_ptr, payload_len) {
                    Some(b) => bytes::Bytes::copy_from_slice(b),
                    None => {
                        results[0] = wasmtime::Val::I32(-1);
                        return Ok(());
                    }
                };
                let nats = caller.data().nats.clone();
                let _ = data;
                match nats {
                    None => {
                        results[0] = wasmtime::Val::I32(-1);
                    }
                    Some(nc) => match NatsBroker::publish(&nc, subject.into(), payload_bytes).await
                    {
                        Ok(_) => {
                            results[0] = wasmtime::Val::I32(0);
                        }
                        Err(_) => {
                            results[0] = wasmtime::Val::I32(-1);
                        }
                    },
                }
                Ok(())
            })
        },
    )?;

    // trogon_v1.nats_request(subj_ptr, subj_len, pay_ptr, pay_len, timeout_ms, out_ptr, out_max) -> i32
    // timeout_ms: > 0 → wait that many ms; <= 0 → default 30s timeout.
    // Note: unlike recv_message (where 0 means non-blocking poll), here 0 means "use default".
    // Returns bytes written to out_ptr on success, -1 on error/timeout/no NATS.
    linker.func_new_async(
        "trogon_v1",
        "nats_request",
        wasmtime::FuncType::new(
            engine,
            [
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I32,
                ValType::I32,
            ],
            [ValType::I32],
        ),
        |mut caller: Caller<'_, WasmStoreData<N>>, params, results| {
            let subj_ptr = params[0].unwrap_i32() as usize;
            let subj_len = params[1].unwrap_i32() as usize;
            let pay_ptr = params[2].unwrap_i32() as usize;
            let pay_len = params[3].unwrap_i32() as usize;
            let timeout_ms = params[4].unwrap_i32();
            let out_ptr = params[5].unwrap_i32() as usize;
            let out_max = params[6].unwrap_i32() as usize;
            Box::new(async move {
                // Check and decrement host call budget; return -1 if exhausted.
                if caller.data().host_call_budget == 0 {
                    results[0] = wasmtime::Val::I32(-1);
                    return Ok(());
                }
                caller.data_mut().host_call_budget -= 1;
                METRICS
                    .host_calls_total
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                let mem = match caller.get_export("memory").and_then(|e| {
                    if let Extern::Memory(m) = e {
                        Some(m)
                    } else {
                        None
                    }
                }) {
                    Some(m) => m,
                    None => {
                        results[0] = wasmtime::Val::I32(-1);
                        return Ok(());
                    }
                };

                // Read subject and payload BEFORE any await.
                let (subject, payload) = {
                    let data = mem.data(&caller);
                    let subj = match read_str(data, subj_ptr, subj_len) {
                        Some(s) => s.to_owned(),
                        None => {
                            results[0] = wasmtime::Val::I32(-1);
                            return Ok(());
                        }
                    };
                    let pay = match read_bytes(data, pay_ptr, pay_len) {
                        Some(b) => bytes::Bytes::copy_from_slice(b),
                        None => {
                            results[0] = wasmtime::Val::I32(-1);
                            return Ok(());
                        }
                    };
                    (subj, pay)
                };

                let nats = match caller.data().nats.clone() {
                    Some(n) => n,
                    None => {
                        results[0] = wasmtime::Val::I32(-1);
                        return Ok(());
                    }
                };

                let timeout = if timeout_ms <= 0 {
                    std::time::Duration::from_secs(30) // default 30s when not specified
                } else {
                    std::time::Duration::from_millis(timeout_ms as u64)
                };
                let response = match tokio::time::timeout(
                    timeout,
                    NatsBroker::request(&nats, subject, payload),
                )
                .await
                {
                    Ok(Ok(msg)) => msg.payload,
                    _ => {
                        results[0] = wasmtime::Val::I32(-1);
                        return Ok(());
                    }
                };

                // Write response into WASM memory.
                let to_write = response.len().min(out_max);
                let mem_data = mem.data_mut(&mut caller);
                if out_ptr + to_write > mem_data.len() {
                    results[0] = wasmtime::Val::I32(-1);
                    return Ok(());
                }
                mem_data[out_ptr..out_ptr + to_write].copy_from_slice(&response[..to_write]);
                results[0] = wasmtime::Val::I32(to_write as i32);
                Ok(())
            })
        },
    )?;

    // trogon_v1.subscribe(subject_ptr, subject_len) -> i32
    // Returns subscription ID (>= 0) or -1 on error/no NATS.
    linker.func_new_async(
        "trogon_v1",
        "subscribe",
        wasmtime::FuncType::new(engine, [ValType::I32, ValType::I32], [ValType::I32]),
        |mut caller: Caller<'_, WasmStoreData<N>>, params, results| {
            let subj_ptr = params[0].unwrap_i32() as usize;
            let subj_len = params[1].unwrap_i32() as usize;
            Box::new(async move {
                // Check and decrement host call budget; return -1 if exhausted.
                if caller.data().host_call_budget == 0 {
                    results[0] = wasmtime::Val::I32(-1);
                    return Ok(());
                }
                caller.data_mut().host_call_budget -= 1;
                METRICS
                    .host_calls_total
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                // Enforce per-module subscription cap to prevent NATS connection exhaustion.
                if caller.data().subscriptions.len() >= MAX_SUBSCRIPTIONS_PER_MODULE {
                    results[0] = wasmtime::Val::I32(-1);
                    return Ok(());
                }

                let mem = match caller.get_export("memory").and_then(|e| {
                    if let Extern::Memory(m) = e {
                        Some(m)
                    } else {
                        None
                    }
                }) {
                    Some(m) => m,
                    None => {
                        results[0] = wasmtime::Val::I32(-1);
                        return Ok(());
                    }
                };

                let subject = {
                    let data = mem.data(&caller);
                    match read_str(data, subj_ptr, subj_len) {
                        Some(s) => s.to_owned(),
                        None => {
                            results[0] = wasmtime::Val::I32(-1);
                            return Ok(());
                        }
                    }
                };

                let nats = match caller.data().nats.clone() {
                    Some(n) => n,
                    None => {
                        results[0] = wasmtime::Val::I32(-1);
                        return Ok(());
                    }
                };

                let sub = match NatsBroker::subscribe(&nats, &subject).await {
                    Ok(s) => s,
                    Err(_) => {
                        results[0] = wasmtime::Val::I32(-1);
                        return Ok(());
                    }
                };

                let id = caller.data().next_sub_id;
                caller.data_mut().next_sub_id += 1;
                caller.data_mut().subscriptions.insert(id, sub);
                results[0] = wasmtime::Val::I32(id);
                Ok(())
            })
        },
    )?;

    // trogon.recv_message(sub_id,
    //                     out_subj_ptr, out_subj_max, out_subj_len_ptr,
    //                     out_payload_ptr, out_payload_max, out_payload_len_ptr,
    //                     timeout_ms) -> i32
    //
    // On success (1): writes subject bytes into out_subj_ptr and actual subject
    // length (i32 LE) into out_subj_len_ptr; writes payload bytes into
    // out_payload_ptr and actual payload length (i32 LE) into out_payload_len_ptr.
    // Returns 0 on timeout / subscription closed, -1 on error.
    //
    // Separate length output pointers replace the old opaque sum return value so
    // that callers can tell exactly how many bytes were written to each buffer.
    linker.func_new_async(
        "trogon_v1",
        "recv_message",
        wasmtime::FuncType::new(
            engine,
            [
                ValType::I32, // sub_id
                ValType::I32, // out_subj_ptr
                ValType::I32, // out_subj_max
                ValType::I32, // out_subj_len_ptr
                ValType::I32, // out_payload_ptr
                ValType::I32, // out_payload_max
                ValType::I32, // out_payload_len_ptr
                ValType::I32, // timeout_ms
            ],
            [ValType::I32],
        ),
        |mut caller: Caller<'_, WasmStoreData<N>>, params, results| {
            let sub_id = params[0].unwrap_i32();
            let out_subj_ptr = params[1].unwrap_i32() as usize;
            let out_subj_max = params[2].unwrap_i32() as usize;
            let out_subj_len_ptr = params[3].unwrap_i32() as usize;
            let out_payload_ptr = params[4].unwrap_i32() as usize;
            let out_payload_max = params[5].unwrap_i32() as usize;
            let out_payload_len_ptr = params[6].unwrap_i32() as usize;
            let timeout_ms = params[7].unwrap_i32();
            Box::new(async move {
                // Check and decrement host call budget first.
                if caller.data().host_call_budget == 0 {
                    results[0] = wasmtime::Val::I32(-1);
                    return Ok(());
                }
                caller.data_mut().host_call_budget -= 1;
                METRICS
                    .host_calls_total
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                // Take the subscriber out temporarily for async recv.
                let mut sub = match caller.data_mut().subscriptions.remove(&sub_id) {
                    Some(s) => s,
                    None => {
                        results[0] = wasmtime::Val::I32(-1);
                        return Ok(());
                    }
                };

                let timeout = if timeout_ms < 0 {
                    std::time::Duration::from_secs(30) // default 30s
                } else if timeout_ms == 0 {
                    std::time::Duration::from_millis(1) // non-blocking: poll
                } else {
                    std::time::Duration::from_millis(timeout_ms as u64)
                };
                let result = tokio::time::timeout(timeout, sub.next()).await;

                // Put subscriber back.
                caller.data_mut().subscriptions.insert(sub_id, sub);

                let msg = match result {
                    Ok(Some(m)) => m,
                    Ok(None) | Err(_) => {
                        // Closed or timeout.
                        results[0] = wasmtime::Val::I32(0);
                        return Ok(());
                    }
                };

                let subj_bytes = msg.subject.as_str().as_bytes().to_vec();
                let payload_bytes = msg.payload.clone();

                let mem = match caller.get_export("memory").and_then(|e| {
                    if let Extern::Memory(m) = e {
                        Some(m)
                    } else {
                        None
                    }
                }) {
                    Some(m) => m,
                    None => {
                        results[0] = wasmtime::Val::I32(-1);
                        return Ok(());
                    }
                };

                let subj_to_write = subj_bytes.len().min(out_subj_max);
                let payload_to_write = payload_bytes.len().min(out_payload_max);

                let mem_data = mem.data_mut(&mut caller);

                // Validate all writes fit in WASM memory before touching it.
                let subj_end = out_subj_ptr.saturating_add(subj_to_write);
                let payload_end = out_payload_ptr.saturating_add(payload_to_write);
                let subj_len_end = out_subj_len_ptr.saturating_add(4);
                let payload_len_end = out_payload_len_ptr.saturating_add(4);
                if subj_end > mem_data.len()
                    || payload_end > mem_data.len()
                    || subj_len_end > mem_data.len()
                    || payload_len_end > mem_data.len()
                {
                    results[0] = wasmtime::Val::I32(-1);
                    return Ok(());
                }

                mem_data[out_subj_ptr..subj_end].copy_from_slice(&subj_bytes[..subj_to_write]);
                mem_data[out_payload_ptr..payload_end]
                    .copy_from_slice(&payload_bytes[..payload_to_write]);
                mem_data[out_subj_len_ptr..subj_len_end]
                    .copy_from_slice(&(subj_to_write as i32).to_le_bytes());
                mem_data[out_payload_len_ptr..payload_len_end]
                    .copy_from_slice(&(payload_to_write as i32).to_le_bytes());

                results[0] = wasmtime::Val::I32(1);
                Ok(())
            })
        },
    )?;

    // trogon.unsubscribe(sub_id) -> i32
    // Returns 0 on success, -1 if sub_id not found.
    linker.func_new_async(
        "trogon_v1",
        "unsubscribe",
        wasmtime::FuncType::new(engine, [ValType::I32], [ValType::I32]),
        |mut caller: Caller<'_, WasmStoreData<N>>, params, results| {
            let sub_id = params[0].unwrap_i32();
            Box::new(async move {
                // Check and decrement host call budget; return -1 if exhausted.
                if caller.data().host_call_budget == 0 {
                    results[0] = wasmtime::Val::I32(-1);
                    return Ok(());
                }
                caller.data_mut().host_call_budget -= 1;
                METRICS
                    .host_calls_total
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                if caller.data_mut().subscriptions.remove(&sub_id).is_some() {
                    results[0] = wasmtime::Val::I32(0);
                } else {
                    results[0] = wasmtime::Val::I32(-1);
                }
                Ok(())
            })
        },
    )?;

    // trogon_v1.request_permission(options_json_ptr, options_json_len, out_selected_ptr) -> i32
    // Returns 0 on success (writes selected index to out_selected_ptr), -1 on cancel/error.
    linker.func_new_async(
        "trogon_v1",
        "request_permission",
        wasmtime::FuncType::new(
            engine,
            [ValType::I32, ValType::I32, ValType::I32],
            [ValType::I32],
        ),
        |mut caller: Caller<'_, WasmStoreData<N>>, params, results| {
            let opt_ptr = params[0].unwrap_i32() as usize;
            let opt_len = params[1].unwrap_i32() as usize;
            let out_ptr = params[2].unwrap_i32() as usize;
            Box::new(async move {
                // Check and decrement host call budget; return -1 if exhausted.
                if caller.data().host_call_budget == 0 {
                    results[0] = wasmtime::Val::I32(-1);
                    return Ok(());
                }
                caller.data_mut().host_call_budget -= 1;
                METRICS.host_calls_total.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

                let mem = match caller.get_export("memory").and_then(|e| {
                    if let Extern::Memory(m) = e { Some(m) } else { None }
                }) {
                    Some(m) => m,
                    None => { results[0] = wasmtime::Val::I32(-1); return Ok(()); }
                };

                let (options, auto_allow, nats, session_id, acp_prefix) = {
                    let data = mem.data(&caller);
                    let json_str = match read_str(data, opt_ptr, opt_len) {
                        Some(s) => s.to_owned(),
                        None => { results[0] = wasmtime::Val::I32(-1); return Ok(()); }
                    };
                    let opts: Vec<String> = serde_json::from_str(&json_str).unwrap_or_default();
                    let auto = caller.data().auto_allow_permissions;
                    let nats = caller.data().nats.clone();
                    let sid = caller.data().session_id.clone();
                    let prefix = caller.data().acp_prefix.clone();
                    (opts, auto, nats, sid, prefix)
                };

                if options.is_empty() {
                    results[0] = wasmtime::Val::I32(-1);
                    return Ok(());
                }

                if auto_allow {
                    // Write selected index (0) to WASM memory.
                    let mem_data = mem.data_mut(&mut caller);
                    if out_ptr + 4 <= mem_data.len() {
                        mem_data[out_ptr..out_ptr + 4].copy_from_slice(&0i32.to_le_bytes());
                    }
                    results[0] = wasmtime::Val::I32(0);
                } else if let Some(nats_client) = nats {
                    // Send a NATS request using the ACP-standard RequestPermissionRequest schema.
                    // Each option string becomes a PermissionOption with optionId = its index
                    // (as a string) and title = the string itself. The toolCall field is a
                    // synthetic entry required by the schema; it identifies the WASM module as
                    // the caller. The response is parsed as RequestPermissionOutcome:
                    //   {"outcome":"selected","optionId":"<index>"} → write index to out_ptr
                    //   {"outcome":"cancelled"}                     → return -1
                    let subject = format!("{acp_prefix}.session.{session_id}.wasm.request_permission");
                    let acp_options: Vec<serde_json::Value> = options
                        .iter()
                        .enumerate()
                        .map(|(i, title)| serde_json::json!({
                            "optionId": i.to_string(),
                            "title": title,
                        }))
                        .collect();
                    let request_json = serde_json::json!({
                        "sessionId": session_id,
                        "toolCall": {
                            "toolCallId": format!("wasm-perm-{session_id}"),
                            "name": "request_permission",
                            "input": {},
                        },
                        "options": acp_options,
                    })
                    .to_string();
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(30),
                        NatsBroker::request(&nats_client, subject, bytes::Bytes::from(request_json)),
                    )
                    .await
                    {
                        Ok(Ok(msg)) => {
                            if let Ok(resp) =
                                serde_json::from_slice::<serde_json::Value>(&msg.payload)
                            {
                                let outcome = resp.get("outcome").and_then(|v| v.as_str()).unwrap_or("");
                                if outcome == "cancelled" {
                                    results[0] = wasmtime::Val::I32(-1);
                                } else if outcome == "selected" {
                                    // optionId is the string representation of the index.
                                    let idx = resp
                                        .get("optionId")
                                        .and_then(|v| v.as_str())
                                        .and_then(|s| s.parse::<i32>().ok());
                                    match idx {
                                        Some(i) if i >= 0 && (i as usize) < options.len() => {
                                            let mem_data = mem.data_mut(&mut caller);
                                            if out_ptr + 4 <= mem_data.len() {
                                                mem_data[out_ptr..out_ptr + 4]
                                                    .copy_from_slice(&i.to_le_bytes());
                                            }
                                            results[0] = wasmtime::Val::I32(0);
                                        }
                                        _ => {
                                            tracing::warn!(session_id = %session_id, "Permission response 'optionId' is missing or out of range");
                                            results[0] = wasmtime::Val::I32(-1);
                                        }
                                    }
                                } else {
                                    tracing::warn!(session_id = %session_id, "Permission response missing or unknown 'outcome' field");
                                    results[0] = wasmtime::Val::I32(-1);
                                }
                            } else {
                                tracing::warn!(session_id = %session_id, "Permission response is not valid JSON");
                                results[0] = wasmtime::Val::I32(-1);
                            }
                        }
                        Ok(Err(e)) => {
                            tracing::warn!(session_id = %session_id, error = %e, "NATS permission request failed");
                            results[0] = wasmtime::Val::I32(-1);
                        }
                        Err(_elapsed) => {
                            tracing::warn!(session_id = %session_id, "NATS permission request timed out after 30s");
                            results[0] = wasmtime::Val::I32(-1);
                        }
                    }
                } else {
                    // No NATS and not auto-allow: deny.
                    results[0] = wasmtime::Val::I32(-1);
                }
                Ok(())
            })
        },
    )?;

    Ok(())
}

// ── RealWasmExecutor ─────────────────────────────────────────────────────────

/// Cache fingerprint combining crate version and wasmtime version.
/// The wasmtime version is injected at build time from Cargo.lock by build.rs,
/// so it updates automatically when the dependency is bumped.
pub(crate) const CACHE_FINGERPRINT: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "+wasmtime-",
    env!("WASMTIME_VERSION")
);

/// Maximum number of compiled modules kept in the per-process in-memory cache.
const MAX_CACHED_MODULES: usize = 64;

/// Derives an on-disk cache key from the absolute path of a `.wasm` file.
///
/// Uses FNV-1a 64-bit, which is deterministic across Rust versions and
/// process restarts. `DefaultHasher` is explicitly avoided because its
/// implementation is "subject to change" between compiler versions, which
/// would silently invalidate on-disk caches after a toolchain upgrade.
pub(crate) fn cache_key(path: &std::path::Path) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in path.as_os_str().as_encoded_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Tuple stored in the in-memory module cache: `(instance_pre, source_mtime, last_accessed)`.
pub(crate) type CachedModuleEntry<N> = (
    InstancePre<WasmStoreData<N>>,
    std::time::SystemTime,
    std::time::Instant,
);

/// Production `WasmExecutor` implementation backed by a real `wasmtime::Engine`.
///
/// Contains the engine, linker, module cache, and cache directory. Handles
/// compiling (or loading from cache) a `.wasm` file and running it via
/// `run_module_compiled`.
pub struct RealWasmExecutor<
    N: NatsBroker + Send + Sync = async_nats::Client,
    CL: Clock = StdClock,
    SF: SyncFs = StdSyncFs,
> {
    engine: Engine,
    linker: Linker<WasmStoreData<N>>,
    modules: Arc<RefCell<HashMap<PathBuf, CachedModuleEntry<N>>>>,
    module_cache_dir: Option<PathBuf>,
    wasm_max_module_size_bytes: usize,
    clock: CL,
    sync_fs: SF,
}

impl<N: NatsBroker + Send + Sync, CL: Clock, SF: SyncFs> Clone for RealWasmExecutor<N, CL, SF> {
    fn clone(&self) -> Self {
        Self {
            engine: self.engine.clone(),
            linker: self.linker.clone(),
            modules: Arc::clone(&self.modules),
            module_cache_dir: self.module_cache_dir.clone(),
            wasm_max_module_size_bytes: self.wasm_max_module_size_bytes,
            clock: self.clock.clone(),
            sync_fs: self.sync_fs.clone(),
        }
    }
}

impl<N: NatsBroker + Send + Sync + 'static> RealWasmExecutor<N, StdClock, StdSyncFs> {
    /// Creates a new executor from a [`Config`].
    pub fn new(config: &Config) -> Result<Self, wasmtime::Error> {
        Self::new_with(config, StdClock, StdSyncFs)
    }
}

impl<N: NatsBroker + Send + Sync + 'static, CL: Clock, SF: SyncFs> RealWasmExecutor<N, CL, SF> {
    /// Creates a new executor from a [`Config`] with custom clock and sync fs.
    pub fn new_with(config: &Config, clock: CL, sync_fs: SF) -> Result<Self, wasmtime::Error> {
        let mut wasm_config = wasmtime::Config::new();
        wasm_config.async_support(true);
        wasm_config.consume_fuel(true);
        let engine = Engine::new(&wasm_config)?;
        let linker = build_linker::<N>(&engine)?;
        Ok(Self {
            engine,
            linker,
            modules: Arc::new(RefCell::new(HashMap::new())),
            module_cache_dir: config.module_cache_dir.clone(),
            wasm_max_module_size_bytes: config.wasm_max_module_size_bytes,
            clock,
            sync_fs,
        })
    }

    /// Returns a compiled `InstancePre` for `wasm_path`, using in-memory and
    /// on-disk caches. Invalidates both caches when the source file's mtime changes.
    ///
    /// Disk I/O and Cranelift compilation run inside `spawn_blocking` so the
    /// tokio `LocalSet` is not stalled during CPU-intensive JIT compilation.
    async fn get_or_compile_module(
        &self,
        wasm_path: &std::path::Path,
    ) -> Result<InstancePre<WasmStoreData<N>>, anyhow::Error> {
        let abs_path = wasm_path
            .canonicalize()
            .unwrap_or_else(|_| wasm_path.to_path_buf());

        // Check file size limit and mtime via blocking I/O.
        let max_size = self.wasm_max_module_size_bytes;
        let abs_path_b = abs_path.clone();
        let sync_fs = self.sync_fs.clone();
        let (meta_len, current_mtime) = tokio::task::spawn_blocking(move || {
            let meta = sync_fs.metadata(&abs_path_b)?;
            Ok::<_, anyhow::Error>((meta.len, meta.modified))
        })
        .await??;

        if max_size > 0 && meta_len as usize > max_size {
            return Err(anyhow::anyhow!(
                "WASM module too large: {} bytes (limit: {})",
                meta_len,
                max_size
            ));
        }

        // Check in-memory cache (synchronous — just a RefCell borrow).
        {
            let mut cached = self.modules.borrow_mut();
            if let Some((pre, mtime, last_accessed)) = cached.get_mut(&abs_path) {
                if *mtime == current_mtime {
                    *last_accessed = self.clock.now();
                    METRICS
                        .cache_hits
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    return Ok(pre.clone());
                }
            }
        }

        // Check on-disk cache and compile if needed — all blocking work in one closure.
        let engine = self.engine.clone();
        let cache_dir = self.module_cache_dir.clone();
        let abs_path_b = abs_path.clone();
        let sync_fs2 = self.sync_fs.clone();
        let m = tokio::task::spawn_blocking(move || -> Result<Module, anyhow::Error> {
            if let Some(ref cache_dir) = cache_dir {
                let key = cache_key(&abs_path_b);
                let cwasm_path = cache_dir.join(format!("{key}.cwasm"));
                let mtime_path = cache_dir.join(format!("{key}.mtime"));
                let version_path = cache_dir.join(format!("{key}.version"));

                if cwasm_path.exists() && mtime_path.exists() {
                    let stored_version = sync_fs2.read_to_string(&version_path).unwrap_or_default();
                    let version_ok = stored_version.trim() == CACHE_FINGERPRINT;

                    if version_ok {
                        if let Ok(mtime_str) = sync_fs2.read_to_string(&mtime_path) {
                            let stored_nanos: u64 = mtime_str.trim().parse().unwrap_or(0);
                            let current_nanos = current_mtime
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_nanos() as u64)
                                .unwrap_or(0);
                            if stored_nanos != 0 && stored_nanos == current_nanos {
                                // SAFETY: the engine config is always identical for cached
                                // modules — same wasmtime::Config, same Cranelift settings.
                                let cached_module =
                                    unsafe { Module::deserialize_file(&engine, &cwasm_path) };
                                match cached_module {
                                    Ok(m) => {
                                        METRICS
                                            .cache_hits
                                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                        return Ok(m);
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            path = %cwasm_path.display(),
                                            error = %e,
                                            "on-disk module cache invalid, recompiling"
                                        );
                                    }
                                }
                            }
                        }
                    } else {
                        tracing::warn!(
                            path = %cwasm_path.display(),
                            stored = %stored_version.trim(),
                            current = %CACHE_FINGERPRINT,
                            "module cache version mismatch, recompiling"
                        );
                    }
                }
            }

            // Compile fresh from source (cache miss).
            METRICS
                .cache_misses
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let m = Module::from_file(&engine, &abs_path_b)?;

            // Write on-disk cache if configured (fire-and-forget; errors are non-fatal).
            if let Some(ref cache_dir) = cache_dir {
                let key = cache_key(&abs_path_b);
                if sync_fs2.create_dir_all(cache_dir).is_ok() {
                    if let Ok(serialized) = m.serialize() {
                        let cwasm_path = cache_dir.join(format!("{key}.cwasm"));
                        let mtime_path = cache_dir.join(format!("{key}.mtime"));
                        let version_path = cache_dir.join(format!("{key}.version"));
                        let current_nanos = current_mtime
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_nanos() as u64)
                            .unwrap_or(0);
                        let _ = sync_fs2.write(&cwasm_path, &serialized);
                        let _ = sync_fs2.write(&mtime_path, current_nanos.to_string().as_bytes());
                        let _ = sync_fs2.write(&version_path, CACHE_FINGERPRINT.as_bytes());
                    }
                }
            }

            Ok(m)
        })
        .await??;

        // Build InstancePre on the LocalSet thread — it is !Send and cannot be
        // created inside spawn_blocking. This call is fast (import resolution only;
        // Cranelift compilation already happened in the spawn_blocking above).
        let pre = self.linker.instantiate_pre(&m)?;
        {
            let mut cache = self.modules.borrow_mut();
            // Evict the least-recently-accessed (LRU) entry when at capacity.
            if cache.len() >= MAX_CACHED_MODULES && !cache.contains_key(&abs_path) {
                if let Some(lru) = cache
                    .iter()
                    .min_by_key(|(_, (_, _, last_accessed))| *last_accessed)
                    .map(|(k, _)| k.clone())
                {
                    cache.remove(&lru);
                }
            }
            cache.insert(abs_path, (pre.clone(), current_mtime, self.clock.now()));
        }
        Ok(pre)
    }
}

impl<N: NatsBroker + Send + Sync + 'static, CL: Clock, SF: SyncFs> WasmExecutor<N>
    for RealWasmExecutor<N, CL, SF>
{
    fn validate(&self, wasm_path: &std::path::Path) -> Result<(), anyhow::Error> {
        let meta = self
            .sync_fs
            .metadata(wasm_path)
            .map_err(|e| anyhow::anyhow!("{}: {e}", wasm_path.display()))?;
        let max_size = self.wasm_max_module_size_bytes;
        if max_size > 0 && meta.len as usize > max_size {
            return Err(anyhow::anyhow!(
                "WASM module too large: {} bytes (limit: {})",
                meta.len,
                max_size
            ));
        }
        Ok(())
    }

    async fn run(&self, config: WasmRunConfig<N>) -> Result<TerminalExitStatus, anyhow::Error> {
        let instance_pre = self.get_or_compile_module(&config.wasm_path).await?;
        let cwd = if config.cwd.as_deref().unwrap_or(&config.sandbox_dir) != config.sandbox_dir {
            config.cwd
        } else {
            None
        };
        run_module_compiled(WasmExecConfig {
            instance_pre,
            argv: config.argv,
            env_vars: config.env_vars,
            sandbox_dir: config.sandbox_dir,
            cwd,
            output_buf: config.output_buf,
            was_truncated: config.was_truncated,
            output_byte_limit: config.output_byte_limit,
            timeout_secs: config.timeout_secs,
            memory_limit_bytes: config.memory_limit_bytes,
            nats_client: config.nats_client,
            session_id: config.session_id,
            allow_network: config.allow_network,
            auto_allow_permissions: config.auto_allow_permissions,
            fuel_limit: config.fuel_limit,
            host_call_limit: config.host_call_limit,
            acp_prefix: config.acp_prefix,
        })
        .await
    }
}

// ── Module execution ─────────────────────────────────────────────────────────

/// Runs a WASIp1 core module from a pre-linked `InstancePre`.
///
/// All execution parameters are passed via [`WasmExecConfig`].
/// The module is sandboxed to `config.sandbox_dir` as its preopened root (`/`).
/// If `config.cwd` differs from `sandbox_dir`, it is additionally preopened at
/// `"."` so that WASIp1 programs using `getcwd()` / relative paths see the
/// correct working directory.
/// Output is appended directly to `config.output_buf` as bytes arrive.
pub(crate) async fn run_module_compiled<N: NatsBroker + Send + Sync + 'static>(
    config: WasmExecConfig<N>,
) -> Result<TerminalExitStatus, anyhow::Error> {
    let WasmExecConfig {
        instance_pre,
        argv,
        env_vars,
        sandbox_dir,
        cwd,
        output_buf,
        was_truncated,
        output_byte_limit,
        timeout_secs,
        memory_limit_bytes,
        nats_client,
        session_id,
        allow_network,
        auto_allow_permissions,
        fuel_limit,
        host_call_limit,
        acp_prefix,
    } = config;

    // Set up streaming stdout/stderr via duplex pipes.
    let (stdout_writer, mut stdout_reader) = tokio::io::duplex(64 * 1024);
    let (stderr_writer, mut stderr_reader) = tokio::io::duplex(64 * 1024);

    let stdout_stream = AsyncStdoutStream::new(AsyncWriteStream::new(64 * 1024, stdout_writer));
    let stderr_stream = AsyncStdoutStream::new(AsyncWriteStream::new(64 * 1024, stderr_writer));

    let mut builder = WasiCtxBuilder::new();
    builder
        .stdout(stdout_stream)
        .stderr(stderr_stream)
        .args(&argv)
        .preopened_dir(&sandbox_dir, "/", DirPerms::all(), FilePerms::all())?;

    // Pre-open the requested working directory at "." so that WASIp1 programs
    // using getcwd() or relative paths see the correct working directory.
    if let Some(ref cwd_path) = cwd {
        if cwd_path != &sandbox_dir {
            builder.preopened_dir(cwd_path, ".", DirPerms::all(), FilePerms::all())?;
        }
    }

    for (k, v) in env_vars {
        builder.env(k, v);
    }

    if allow_network {
        builder.inherit_network();
    }

    let wasi_ctx = builder.build_p1();
    let store_data = WasmStoreData {
        wasi: wasi_ctx,
        limits: StoreLimitsData {
            memory_limit: memory_limit_bytes,
        },
        nats: nats_client,
        session_id,
        auto_allow_permissions,
        // 0 means unlimited (same convention as wasm_fuel_limit).
        host_call_budget: if host_call_limit == 0 {
            u32::MAX
        } else {
            host_call_limit
        },
        subscriptions: HashMap::new(),
        next_sub_id: 0,
        acp_prefix,
    };
    let mut store = Store::new(instance_pre.module().engine(), store_data);

    // Wire up the memory limiter if a limit is set.
    if memory_limit_bytes.is_some() {
        store.limiter(|data| data as &mut dyn ResourceLimiter);
    }

    // Enable fuel consumption. 0 means unlimited (use u64::MAX); otherwise use the
    // configured limit. The engine has consume_fuel(true) enabled via Config.
    let effective_fuel = if fuel_limit == 0 {
        u64::MAX
    } else {
        fuel_limit
    };
    store.set_fuel(effective_fuel)?;
    // Yield to the async executor every 10 000 fuel units so that wall-clock
    // timeouts (tokio::time::timeout wrapping call_async) can fire even for
    // pure-compute WASM loops that make no async host calls.
    store.fuel_async_yield_interval(Some(10_000))?;

    // Spawn reader tasks BEFORE running the module so output streams concurrently.
    let buf_out = Arc::clone(&output_buf);
    let trunc_out = Arc::clone(&was_truncated);
    let limit_out = output_byte_limit;
    let stdout_task = tokio::task::spawn(async move {
        let mut chunk = [0u8; 4096];
        loop {
            match stdout_reader.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    crate::terminal::append_output(&buf_out, &trunc_out, limit_out, &chunk[..n])
                }
            }
        }
    });

    let buf_err = Arc::clone(&output_buf);
    let trunc_err = Arc::clone(&was_truncated);
    let stderr_task = tokio::task::spawn(async move {
        let mut chunk = [0u8; 4096];
        loop {
            match stderr_reader.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    crate::terminal::append_output(&buf_err, &trunc_err, limit_out, &chunk[..n])
                }
            }
        }
    });

    let instance = instance_pre.instantiate_async(&mut store).await?;
    let start = instance.get_typed_func::<(), ()>(&mut store, "_start")?;

    let call_result = if let Some(secs) = timeout_secs {
        match tokio::time::timeout(
            std::time::Duration::from_secs(secs),
            start.call_async(&mut store, ()),
        )
        .await
        {
            Ok(r) => r,
            Err(_elapsed) => {
                // Timeout — drop store to flush writers, then drain reader tasks.
                drop(store);
                let _ = tokio::join!(stdout_task, stderr_task);
                return Ok(TerminalExitStatus::new().signal(Some("timeout".to_string())));
            }
        }
    } else {
        start.call_async(&mut store, ()).await
    };

    let exit_status = match call_result {
        Ok(()) => TerminalExitStatus::new().exit_code(Some(0u32)),
        Err(e) => {
            // Check for fuel exhaustion (OutOfFuel trap) first.
            if let Some(trap) = e.downcast_ref::<wasmtime::Trap>() {
                if *trap == wasmtime::Trap::OutOfFuel {
                    return Ok(TerminalExitStatus::new().signal(Some("fuel_exhausted".to_string())));
                }
            }
            // proc_exit raises I32Exit. The top-level anyhow::Error wraps it
            // (possibly with a WasmBacktrace context), so downcast_ref on the
            // anyhow::Error directly peels through context layers correctly.
            let exit_code = e
                .downcast_ref::<wasmtime_wasi::I32Exit>()
                .map(|x| x.0 as u32);
            match exit_code {
                Some(code) => TerminalExitStatus::new().exit_code(Some(code)),
                None => TerminalExitStatus::new().signal(Some(format!("trap: {e}"))),
            }
        }
    };

    // Drop store to drop WASI context → drops AsyncWriteStream → sends EOF to readers.
    drop(store);
    let _ = tokio::join!(stdout_task, stderr_task);

    Ok(exit_status)
}
