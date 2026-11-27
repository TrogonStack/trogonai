//! Runs a WASM guest through Wasmtime's WASI Preview 2 (Component Model) support, in a
//! deny-by-default sandbox: by default there are no preopened directories and no permitted
//! outbound sockets. Only stdout and stderr are captured, via in-memory pipes. `memory_mib`
//! becomes a real linear-memory ceiling enforced by `StoreLimits`; `timeout` is wall-clock
//! only, enforced with Wasmtime's epoch interruption (a watchdog bumps the engine epoch
//! from another thread, since Wasmtime has no per-instruction CPU-time accounting to bill).
//!
//! A caller may opt into two narrowly scoped, strictly additive capability grants through
//! [`WasmCapabilityGrant`]: preopened directories (read-only or read-write) and an
//! allow-list of `host:port` outbound-socket destinations. Neither is honored unless
//! requested.
//!
//! Guests must be WASM components. Passing a plain core module fails fast with a
//! "failed to parse WebAssembly module" error rather than running under another ABI.

use std::future::Future;
use std::net::{SocketAddr, ToSocketAddrs};
use std::pin::Pin;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use trogon_sandbox_contract::{MountAccess, OutputStream, WasmCapabilityGrant};
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store, StoreLimits, StoreLimitsBuilder, Trap};
use wasmtime_wasi::p2::bindings::sync::Command;
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
use wasmtime_wasi::sockets::SocketAddrUse;
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

use crate::constants::{BYTES_PER_MIB, CAPTURE_BUFFER_BYTES, TRAP_RETRY};
use crate::{CapturedOutput, GuestOutcome, WasmRuntimeError};

type SocketCheck = Pin<Box<dyn Future<Output = bool> + Send + Sync>>;

struct StoreState {
    ctx: WasiCtx,
    table: ResourceTable,
    limits: StoreLimits,
}

impl WasiView for StoreState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.ctx,
            table: &mut self.table,
        }
    }
}

/// Runs the component in `module_bytes` to an outcome, stopping it at `timeout` or when
/// `cancel` fires.
pub async fn run_wasm(
    module_bytes: Vec<u8>,
    memory_mib: u32,
    timeout: Duration,
    cancel: CancellationToken,
    grant: Option<WasmCapabilityGrant>,
) -> Result<GuestOutcome, WasmRuntimeError> {
    let mut config = Config::new();
    config.epoch_interruption(true);
    config.wasm_component_model(true);
    let engine = Engine::new(&config).map_err(WasmRuntimeError::setup)?;

    let watchdog_engine = engine.clone();
    let mut join = tokio::task::spawn_blocking(move || {
        execute_component(&watchdog_engine, &module_bytes, memory_mib, grant.as_ref())
    });

    let stopped: fn(CapturedOutput) -> GuestOutcome = tokio::select! {
        result = &mut join => return result.map_err(|e| WasmRuntimeError::TaskPanicked(e.to_string()))?,
        () = tokio::time::sleep(timeout) => GuestOutcome::timed_out,
        () = cancel.cancelled() => GuestOutcome::cancelled,
    };
    // Wait for the blocking task to observe the trap before reporting, so the guest's thread
    // never outlives the call.
    force_epoch_trap(&engine, &mut join).await;
    Ok(stopped(CapturedOutput::default()))
}

/// Repeatedly bumps `engine`'s epoch until `join` completes.
///
/// A single increment can land before the guest thread sets its epoch deadline, which is
/// relative to the epoch at that moment, leaving the deadline unreachable. Repeating the
/// increment guarantees one lands after the deadline was set.
async fn force_epoch_trap<T>(engine: &Engine, join: &mut tokio::task::JoinHandle<T>) {
    let mut finished = false;
    while !finished {
        engine.increment_epoch();
        finished = tokio::time::timeout(TRAP_RETRY, &mut *join).await.is_ok();
    }
}

/// Resolves each egress rule's hostname to concrete addresses up front: the socket check
/// only observes resolved addresses from inside the guest, with no hostname visibility, so
/// the worker resolves hostnames itself and matches connection attempts against that set.
fn resolve_allowed_addrs(grant: &WasmCapabilityGrant) -> Result<Vec<SocketAddr>, WasmRuntimeError> {
    let mut addrs = Vec::new();
    for rule in &grant.allowed_egress {
        let resolved = (rule.host(), rule.port().get())
            .to_socket_addrs()
            .map_err(|e| WasmRuntimeError::Setup(format!("failed to resolve egress host '{}': {e}", rule.host())))?;
        addrs.extend(resolved);
    }
    Ok(addrs)
}

/// An egress grant is outbound only. The allow-list is matched against `TcpConnect` alone,
/// so an allowed address that happens to be local never lets the guest bind, listen on, or
/// accept from it. The one bind permitted is the unspecified address with port 0, which is
/// the implicit ephemeral bind that precedes a connect. Listening is refused outright.
fn egress_use_allowed(allowed_addrs: &[SocketAddr], addr: SocketAddr, use_: SocketAddrUse) -> bool {
    match use_ {
        SocketAddrUse::TcpConnect => allowed_addrs.contains(&addr),
        SocketAddrUse::TcpBind => addr.ip().is_unspecified() && addr.port() == 0,
        _ => false,
    }
}

fn egress_check(
    allowed_addrs: Vec<SocketAddr>,
) -> impl Fn(SocketAddr, SocketAddrUse) -> SocketCheck + Send + Sync + 'static {
    move |addr, use_| {
        let allowed = egress_use_allowed(&allowed_addrs, addr, use_);
        Box::pin(async move { allowed })
    }
}

fn preopen_perms(access: MountAccess) -> (DirPerms, FilePerms) {
    match access {
        MountAccess::ReadOnly => (DirPerms::READ, FilePerms::READ),
        MountAccess::ReadWrite => (DirPerms::all(), FilePerms::all()),
    }
}

fn apply_capability_grant(builder: &mut WasiCtxBuilder, grant: &WasmCapabilityGrant) -> Result<(), WasmRuntimeError> {
    for preopen in &grant.preopens {
        let (dir_perms, file_perms) = preopen_perms(preopen.access());
        builder
            .preopened_dir(preopen.host_dir(), preopen.guest_path(), dir_perms, file_perms)
            .map_err(WasmRuntimeError::setup)?;
    }

    if !grant.allowed_egress.is_empty() {
        let allowed_addrs = resolve_allowed_addrs(grant)?;
        builder.allow_ip_name_lookup(true);
        // Only TCP is granted, and every connect is still filtered by the address check.
        builder.allow_tcp(true);
        builder.socket_addr_check(egress_check(allowed_addrs));
    }
    Ok(())
}

fn execute_component(
    engine: &Engine,
    module_bytes: &[u8],
    memory_mib: u32,
    grant: Option<&WasmCapabilityGrant>,
) -> Result<GuestOutcome, WasmRuntimeError> {
    let component = Component::from_binary(engine, module_bytes).map_err(WasmRuntimeError::setup)?;

    let mut linker: Linker<StoreState> = Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker).map_err(WasmRuntimeError::setup)?;

    let stdout = MemoryOutputPipe::new(CAPTURE_BUFFER_BYTES);
    let stderr = MemoryOutputPipe::new(CAPTURE_BUFFER_BYTES);

    let mut builder = WasiCtxBuilder::new();
    builder.stdout(stdout.clone()).stderr(stderr.clone());
    grant.map_or(Ok(()), |grant| apply_capability_grant(&mut builder, grant))?;
    let ctx = builder.build();

    let max_bytes = (memory_mib as usize).saturating_mul(BYTES_PER_MIB);
    let limits = StoreLimitsBuilder::new().memory_size(max_bytes).build();
    let mut store = Store::new(
        engine,
        StoreState {
            ctx,
            table: ResourceTable::new(),
            limits,
        },
    );
    store.limiter(|state| &mut state.limits);
    store.set_epoch_deadline(1);
    store.epoch_deadline_trap();

    let outcome = run_command(&mut store, &component, &linker);
    let output = drain_output(&stdout, &stderr);
    Ok(match outcome {
        Ok(()) => GuestOutcome::Completed { output },
        Err(error) if error.downcast_ref::<Trap>() == Some(&Trap::Interrupt) => GuestOutcome::timed_out(output),
        Err(error) => GuestOutcome::failed(output, error.to_string()),
    })
}

fn run_command(
    store: &mut Store<StoreState>,
    component: &Component,
    linker: &Linker<StoreState>,
) -> wasmtime::Result<()> {
    let command = Command::instantiate(&mut *store, component, linker)?;
    command
        .wasi_cli_run()
        .call_run(&mut *store)?
        .map_err(|()| wasmtime::format_err!("guest program exited with a non-zero status"))
}

fn drain_output(stdout: &MemoryOutputPipe, stderr: &MemoryOutputPipe) -> CapturedOutput {
    let streams = [
        (OutputStream::Stdout, stdout.contents()),
        (OutputStream::Stderr, stderr.contents()),
    ];
    let lines = streams
        .into_iter()
        .filter(|(_, bytes)| !bytes.is_empty())
        .map(|(stream, bytes)| (stream, String::from_utf8_lossy(&bytes).into_owned()))
        .collect();
    CapturedOutput { lines }
}

#[cfg(test)]
mod tests;
