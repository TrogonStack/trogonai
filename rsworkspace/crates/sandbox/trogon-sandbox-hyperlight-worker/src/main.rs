use std::process::ExitCode;

use clap::Parser;
#[cfg(not(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows"))))]
use trogon_sandbox_hyperlight_worker::UnsupportedHypervisor;
#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
use trogon_sandbox_hyperlight_worker::WasmHypervisor;
use trogon_sandbox_hyperlight_worker::{WorkerArgs, serve};

#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
fn backend(args: &WorkerArgs) -> WasmHypervisor {
    WasmHypervisor::new(
        args.guest_input_buffer_bytes,
        args.guest_output_buffer_bytes,
        args.guest_heap_bytes,
    )
}

#[cfg(not(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows"))))]
fn backend(_args: &WorkerArgs) -> UnsupportedHypervisor {
    UnsupportedHypervisor
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = WorkerArgs::parse();
    let backend = backend(&args);
    serve(args, backend).await
}
