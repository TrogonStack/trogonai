use std::borrow::Cow;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use deno_core::{Extension, FastString, JsRuntime, OpDecl, OpState, RuntimeOptions, op2, v8};
use trogon_sandbox_contract::OutputStream;

use crate::CapturedOutput;
use crate::constants::{BOOTSTRAP_NAME, BOOTSTRAP_SCRIPT, BYTES_PER_MIB};

#[derive(Default)]
struct PrintSink(Vec<(OutputStream, String)>);

#[op2(fast)]
fn op_captured_print(state: &mut OpState, #[string] msg: &str, is_err: bool) {
    let stream = if is_err {
        OutputStream::Stderr
    } else {
        OutputStream::Stdout
    };
    state.borrow_mut::<PrintSink>().0.push((stream, msg.to_string()));
}

/// Builds a deny-by-default runtime with the print sink installed and the heap capped at
/// `memory_mib`.
pub(crate) fn new_guest_runtime(memory_mib: u32) -> JsRuntime {
    const DECL: OpDecl = op_captured_print();
    let ext = Extension {
        name: "sandbox_v8_wasm_guest",
        ops: Cow::Borrowed(&[DECL]),
        op_state_fn: Some(Box::new(|state: &mut OpState| {
            state.put(PrintSink::default());
        })),
        ..Default::default()
    };
    let max_bytes = (memory_mib as usize).saturating_mul(BYTES_PER_MIB);
    JsRuntime::new(RuntimeOptions {
        extensions: vec![ext],
        create_params: Some(v8::CreateParams::default().heap_limits(0, max_bytes)),
        ..Default::default()
    })
}

/// Stops a script that reaches the heap ceiling instead of letting V8 abort the whole worker
/// process, which is what it does when the limit is hit with no callback installed. The
/// returned flag is set once the ceiling was reached.
pub(crate) fn install_heap_limit_guard(runtime: &mut JsRuntime, handle: v8::IsolateHandle) -> Arc<AtomicBool> {
    let out_of_memory = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&out_of_memory);
    runtime.add_near_heap_limit_callback(move |current_limit, _initial_limit| {
        flag.store(true, Ordering::SeqCst);
        handle.terminate_execution();
        // Headroom so V8 can unwind the terminated script instead of aborting the process.
        current_limit.saturating_add(current_limit / 2)
    });
    out_of_memory
}

pub(crate) fn drain_output(runtime: &mut JsRuntime) -> CapturedOutput {
    let op_state = runtime.op_state();
    let mut state = op_state.borrow_mut();
    let lines = state
        .try_borrow_mut::<PrintSink>()
        .map(|sink| std::mem::take(&mut sink.0))
        .unwrap_or_default();
    CapturedOutput { lines }
}

/// Runs `source` and flattens the engine's error into text.
pub(crate) fn execute(runtime: &mut JsRuntime, name: &'static str, source: FastString) -> Result<(), String> {
    runtime
        .execute_script(name, source)
        .map(drop)
        .map_err(|e| e.to_string())
}

/// Installs the console override and removes ambient authority.
pub(crate) fn bootstrap(runtime: &mut JsRuntime) -> Result<(), String> {
    execute(runtime, BOOTSTRAP_NAME, FastString::from_static(BOOTSTRAP_SCRIPT))
        .map_err(|e| format!("bootstrap failed: {e}"))
}
