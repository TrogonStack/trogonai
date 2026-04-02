use crate::terminal::WasmTerminal;
use agent_client_protocol::TerminalExitStatus;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::io::AsyncReadExt;
use wasmtime::{Caller, Engine, Extern, Linker, Module, ResourceLimiter, Store, ValType};
use wasmtime_wasi::pipe::AsyncWriteStream;
use wasmtime_wasi::preview1::{self, WasiP1Ctx};
use wasmtime_wasi::{AsyncStdoutStream, DirPerms, FilePerms, WasiCtxBuilder};

// ── Store state ─────────────────────────────────────────────────────────────

/// Custom store state that wraps WASI context plus limits and host state.
pub(crate) struct WasmStoreData {
    pub wasi: WasiP1Ctx,
    pub limits: StoreLimitsData,
    pub nats: Option<async_nats::Client>,
    pub session_id: String,
}

/// Memory limit state threaded through the store.
pub(crate) struct StoreLimitsData {
    pub memory_limit: Option<usize>,
}

impl ResourceLimiter for WasmStoreData {
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

fn add_trogon_host_functions(
    engine: &Engine,
    linker: &mut Linker<WasmStoreData>,
) -> anyhow::Result<()> {
    // trogon.log(level_ptr, level_len, msg_ptr, msg_len)
    linker.func_new_async(
        "trogon",
        "log",
        wasmtime::FuncType::new(
            engine,
            [ValType::I32, ValType::I32, ValType::I32, ValType::I32],
            [],
        ),
        |mut caller: Caller<'_, WasmStoreData>, params, _results| {
            let level_ptr = params[0].unwrap_i32() as usize;
            let level_len = params[1].unwrap_i32() as usize;
            let msg_ptr = params[2].unwrap_i32() as usize;
            let msg_len = params[3].unwrap_i32() as usize;
            Box::new(async move {
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

    // trogon.nats_publish(subj_ptr, subj_len, payload_ptr, payload_len) -> i32
    linker.func_new_async(
        "trogon",
        "nats_publish",
        wasmtime::FuncType::new(
            engine,
            [ValType::I32, ValType::I32, ValType::I32, ValType::I32],
            [ValType::I32],
        ),
        |mut caller: Caller<'_, WasmStoreData>, params, results| {
            let subj_ptr = params[0].unwrap_i32() as usize;
            let subj_len = params[1].unwrap_i32() as usize;
            let payload_ptr = params[2].unwrap_i32() as usize;
            let payload_len = params[3].unwrap_i32() as usize;
            Box::new(async move {
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
                    Some(nc) => match nc.publish(subject, payload_bytes).await {
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

    Ok(())
}

// ── Module execution ─────────────────────────────────────────────────────────

/// Runs a WASIp1 core module using a pre-compiled `Module`.
///
/// `argv` must include argv[0] (the program name) as the first element.
/// `env_vars` are set as the WASI environment.
/// The module is sandboxed to `sandbox_dir` as its preopened root (`/`).
/// Output is appended directly to `output_buf` as bytes arrive.
pub async fn run_module_compiled(
    engine: &Engine,
    module: Module,
    argv: &[String],
    env_vars: &[(String, String)],
    sandbox_dir: &Path,
    output_buf: Arc<Mutex<Vec<u8>>>,
    output_byte_limit: usize,
    timeout_secs: Option<u64>,
    memory_limit_bytes: Option<usize>,
    nats_client: Option<async_nats::Client>,
    session_id: String,
) -> Result<TerminalExitStatus, anyhow::Error> {
    let mut linker: Linker<WasmStoreData> = Linker::new(engine);
    preview1::add_to_linker_async(&mut linker, |t| &mut t.wasi)?;
    add_trogon_host_functions(engine, &mut linker)?;
    let pre = linker.instantiate_pre(&module)?;

    // Set up streaming stdout/stderr via duplex pipes.
    let (stdout_writer, mut stdout_reader) = tokio::io::duplex(64 * 1024);
    let (stderr_writer, mut stderr_reader) = tokio::io::duplex(64 * 1024);

    let stdout_stream = AsyncStdoutStream::new(AsyncWriteStream::new(64 * 1024, stdout_writer));
    let stderr_stream = AsyncStdoutStream::new(AsyncWriteStream::new(64 * 1024, stderr_writer));

    let mut builder = WasiCtxBuilder::new();
    builder
        .stdout(stdout_stream)
        .stderr(stderr_stream)
        .args(argv)
        .preopened_dir(sandbox_dir, "/", DirPerms::all(), FilePerms::all())?;

    for (k, v) in env_vars {
        builder.env(k, v);
    }

    let wasi_ctx = builder.build_p1();
    let store_data = WasmStoreData {
        wasi: wasi_ctx,
        limits: StoreLimitsData {
            memory_limit: memory_limit_bytes,
        },
        nats: nats_client,
        session_id,
    };
    let mut store = Store::new(engine, store_data);

    // Wire up the memory limiter if a limit is set.
    if memory_limit_bytes.is_some() {
        store.limiter(|data| data as &mut dyn ResourceLimiter);
    }

    // Enable fuel consumption to prevent runaway modules (1 billion instructions).
    store.set_fuel(1_000_000_000)?;

    // Spawn reader tasks BEFORE running the module so output streams concurrently.
    let buf_out = Arc::clone(&output_buf);
    let limit_out = output_byte_limit;
    let stdout_task = tokio::task::spawn(async move {
        let mut chunk = [0u8; 4096];
        loop {
            match stdout_reader.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => WasmTerminal::append_output(&buf_out, limit_out, &chunk[..n]),
            }
        }
    });

    let buf_err = Arc::clone(&output_buf);
    let stderr_task = tokio::task::spawn(async move {
        let mut chunk = [0u8; 4096];
        loop {
            match stderr_reader.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => WasmTerminal::append_output(&buf_err, limit_out, &chunk[..n]),
            }
        }
    });

    let instance = pre.instantiate_async(&mut store).await?;
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

/// Runs a WASIp1 core module (`.wasm` compiled from `wasm32-wasip1` target).
///
/// The module is sandboxed to `sandbox_dir` as its preopened root (`/`).
/// `args[0]` is set to the module path; remaining `args` are forwarded.
/// `env_vars` are set as the WASI environment.
///
/// Output is appended directly to `output_buf` as the module runs.
#[allow(dead_code)]
pub async fn run_module(
    engine: &Engine,
    wasm_path: &Path,
    args: &[String],
    env_vars: &[(String, String)],
    sandbox_dir: &Path,
    output_buf: Arc<Mutex<Vec<u8>>>,
    output_byte_limit: usize,
    timeout_secs: Option<u64>,
) -> Result<TerminalExitStatus, anyhow::Error> {
    let module = Module::from_file(engine, wasm_path)?;
    let mut argv: Vec<String> = Vec::with_capacity(args.len() + 1);
    argv.push(wasm_path.to_string_lossy().into_owned());
    argv.extend_from_slice(args);
    run_module_compiled(
        engine,
        module,
        &argv,
        env_vars,
        sandbox_dir,
        output_buf,
        output_byte_limit,
        timeout_secs,
        None,
        None,
        String::new(),
    )
    .await
}
