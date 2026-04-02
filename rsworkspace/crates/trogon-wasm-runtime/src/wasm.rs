use agent_client_protocol::TerminalExitStatus;
use std::path::Path;
use wasmtime::{Engine, Linker, Module, Store};
use wasmtime_wasi::preview1::{self, WasiP1Ctx};
use wasmtime_wasi::pipe::MemoryOutputPipe;
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtxBuilder};

/// Output captured from a WASM module execution.
pub struct WasmOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_status: TerminalExitStatus,
}

/// Runs a WASIp1 core module using a pre-compiled `Module`.
///
/// `argv` must include argv[0] (the program name) as the first element.
/// `env_vars` are set as the WASI environment.
/// The module is sandboxed to `sandbox_dir` as its preopened root (`/`).
pub async fn run_module_compiled(
    engine: &Engine,
    module: Module,
    argv: &[String],
    env_vars: &[(String, String)],
    sandbox_dir: &Path,
    output_byte_limit: usize,
    timeout_secs: Option<u64>,
) -> Result<WasmOutput, anyhow::Error> {
    let mut linker: Linker<WasiP1Ctx> = Linker::new(engine);
    preview1::add_to_linker_async(&mut linker, |t| t)?;
    let pre = linker.instantiate_pre(&module)?;

    let stdout_pipe = MemoryOutputPipe::new(output_byte_limit);
    let stderr_pipe = MemoryOutputPipe::new(output_byte_limit);

    let mut builder = WasiCtxBuilder::new();
    builder
        .stdout(stdout_pipe.clone())
        .stderr(stderr_pipe.clone())
        .args(argv)
        .preopened_dir(sandbox_dir, "/", DirPerms::all(), FilePerms::all())?;

    for (k, v) in env_vars {
        builder.env(k, v);
    }

    let wasi_ctx = builder.build_p1();
    let mut store = Store::new(engine, wasi_ctx);

    // Enable fuel consumption to prevent runaway modules (1 billion instructions).
    store.set_fuel(1_000_000_000)?;

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
                // Timeout — return what we have so far.
                let stdout = stdout_pipe.contents().to_vec();
                let stderr = stderr_pipe.contents().to_vec();
                return Ok(WasmOutput {
                    stdout,
                    stderr,
                    exit_status: TerminalExitStatus::new().signal(Some("timeout".to_string())),
                });
            }
        }
    } else {
        start.call_async(&mut store, ()).await
    };

    let exit_status = match call_result {
        Ok(()) => TerminalExitStatus::new().exit_code(Some(0u32)),
        Err(e) => {
            // proc_exit raises I32Exit.  When wasmtime adds a WasmBacktrace
            // context, I32Exit ends up inside an anyhow::Error wrapper whose
            // vtable type is `anyhow::Error`, hiding it from downcast_ref on
            // `dyn Error` chain items.  We try the type-safe path first, then
            // fall back to parsing I32Exit's Display string
            // ("Exited with i32 exit status N").
            // proc_exit raises I32Exit.  The top-level anyhow::Error wraps it
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

    let stdout = stdout_pipe.contents().to_vec();
    let stderr = stderr_pipe.contents().to_vec();

    Ok(WasmOutput {
        stdout,
        stderr,
        exit_status,
    })
}

/// Runs a WASIp1 core module (`.wasm` compiled from `wasm32-wasip1` target).
///
/// The module is sandboxed to `sandbox_dir` as its preopened root (`/`).
/// `args[0]` is set to the module path; remaining `args` are forwarded.
/// `env_vars` are set as the WASI environment.
///
/// stdout and stderr are captured and returned in `WasmOutput`.
/// The function is async because wasmtime async mode is required for
/// well-behaved cooperative scheduling with tokio.
#[allow(dead_code)]
pub async fn run_module(
    engine: &Engine,
    wasm_path: &Path,
    args: &[String],
    env_vars: &[(String, String)],
    sandbox_dir: &Path,
    output_byte_limit: usize,
    timeout_secs: Option<u64>,
) -> Result<WasmOutput, anyhow::Error> {
    let module = Module::from_file(engine, wasm_path)?;
    // Build argv: wasm_path as argv[0], then user-supplied args.
    let mut argv: Vec<String> = Vec::with_capacity(args.len() + 1);
    argv.push(wasm_path.to_string_lossy().into_owned());
    argv.extend_from_slice(args);
    run_module_compiled(
        engine,
        module,
        &argv,
        env_vars,
        sandbox_dir,
        output_byte_limit,
        timeout_secs,
    )
    .await
}
