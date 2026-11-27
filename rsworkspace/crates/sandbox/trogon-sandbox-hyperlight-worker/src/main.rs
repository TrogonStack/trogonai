use std::process::ExitCode;

use clap::Parser;
#[cfg(not(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows"))))]
use trogon_sandbox_hyperlight_worker::UnsupportedHypervisor;
#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
use trogon_sandbox_hyperlight_worker::WasmHypervisor;
use trogon_sandbox_hyperlight_worker::{WorkerArgs, serve};

#[cfg(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows")))]
fn backend() -> WasmHypervisor {
    WasmHypervisor::default()
}

#[cfg(not(all(trogon_hyperlight_wasm, any(target_os = "linux", target_os = "windows"))))]
fn backend() -> UnsupportedHypervisor {
    UnsupportedHypervisor
}

#[tokio::main]
async fn main() -> ExitCode {
    serve(WorkerArgs::parse(), backend()).await
}
