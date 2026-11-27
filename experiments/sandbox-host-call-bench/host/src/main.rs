//! Boundary cost of one guest on three hosts: wasmtime in process, wasmtime in a child process
//! whose host imports are served over pipes, and hyperlight-wasm in a KVM/MSHV micro-VM.
//!
//! Usage: `bench-host all <guest.wasm> [<guest.aot>]` prints a Markdown table.
//! `bench-host child <guest.cwasm>` is the subprocess side and is not meant to be run by hand.

use std::io::{BufReader, BufWriter, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};

const WARMUP_CALLS: usize = 1_000;
const PING_CALLS: usize = 10_000;
const LOOP_COUNT: i32 = 10_000;
const LOOP_RUNS: usize = 50;
const CREATE_RUNS: usize = 100;
const SPAWN_RUNS: usize = 30;
const HL_CREATE_RUNS: usize = 30;
const HL_RESTORE_RUNS: usize = 1_000;

const OP_RESULT: u8 = 0;
const OP_PING: u8 = 1;
const OP_LOOP: u8 = 2;
const OP_HOST_CALL: u8 = 3;

/// Turns any error into an `anyhow::Error` through its `Debug` form, so wasmtime and hyperlight
/// errors convert the same way whatever traits they implement.
trait Ctx<T> {
    fn ctx(self) -> Result<T>;
}

impl<T, E: std::fmt::Debug> Ctx<T> for std::result::Result<T, E> {
    fn ctx(self) -> Result<T> {
        self.map_err(|e| anyhow!("{e:?}"))
    }
}

trait Guest {
    fn ping(&mut self, value: i32) -> Result<i32>;
    fn noop_loop(&mut self, count: i32) -> Result<i32>;
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("child") => child(Path::new(arg(&args, 2)?)),
        Some("all") => {
            let wasm = Path::new(arg(&args, 2)?);
            wasmtime_in_process(wasm)?;
            wasmtime_subprocess(wasm)?;
            hyperlight(args.get(3).map(Path::new))
        }
        _ => bail!("usage: bench-host all <guest.wasm> [<guest.aot>]"),
    }
}

fn arg(args: &[String], index: usize) -> Result<&str> {
    args.get(index)
        .map(String::as_str)
        .ok_or_else(|| anyhow!("missing argument {index}"))
}

// ---------------------------------------------------------------------------------------------
// Reporting

fn row(host: &str, metric: &str, mut samples: Vec<f64>) {
    samples.sort_by(f64::total_cmp);
    let pick = |p: f64| samples[((p / 100.0) * (samples.len() - 1) as f64).round() as usize];
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    println!(
        "| {host} | {metric} | {} | {} | {} | {} |",
        samples.len(),
        fmt(pick(50.0)),
        fmt(pick(99.0)),
        fmt(mean)
    );
}

fn fmt(ns: f64) -> String {
    match ns {
        n if n < 1_000.0 => format!("{n:.0} ns"),
        n if n < 1_000_000.0 => format!("{:.2} µs", n / 1_000.0),
        n => format!("{:.2} ms", n / 1_000_000.0),
    }
}

fn ns(d: Duration) -> f64 {
    d.as_nanos() as f64
}

/// Times `PING_CALLS` single host-to-guest calls, then `LOOP_RUNS` batches of `LOOP_COUNT`
/// guest-to-host calls, reporting each batch as the cost of one call.
fn measure_calls(host: &str, guest: &mut impl Guest) -> Result<()> {
    for i in 0..WARMUP_CALLS as i32 {
        guest.ping(i)?;
    }
    guest.noop_loop(LOOP_COUNT)?;

    let mut ping = Vec::with_capacity(PING_CALLS);
    for i in 0..PING_CALLS as i32 {
        let start = Instant::now();
        let got = guest.ping(i)?;
        ping.push(ns(start.elapsed()));
        if got != i {
            bail!("{host}: ping({i}) returned {got}");
        }
    }
    row(host, "host→guest call (`ping`)", ping);

    let mut per_call = Vec::with_capacity(LOOP_RUNS);
    for _ in 0..LOOP_RUNS {
        let start = Instant::now();
        let got = guest.noop_loop(LOOP_COUNT)?;
        per_call.push(ns(start.elapsed()) / f64::from(LOOP_COUNT));
        if got != LOOP_COUNT {
            bail!("{host}: noop_loop returned {got}");
        }
    }
    row(host, "guest→host call (amortized over 10k)", per_call);
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// wasmtime, in process

struct InProcess {
    store: wasmtime::Store<()>,
    ping: wasmtime::TypedFunc<i32, i32>,
    noop_loop: wasmtime::TypedFunc<i32, i32>,
}

impl InProcess {
    fn new(pre: &wasmtime::InstancePre<()>) -> Result<Self> {
        let mut store = wasmtime::Store::new(pre.module().engine(), ());
        let instance = pre.instantiate(&mut store).ctx()?;
        let ping = instance.get_typed_func(&mut store, "ping").ctx()?;
        let noop_loop = instance.get_typed_func(&mut store, "noop_loop").ctx()?;
        Ok(Self { store, ping, noop_loop })
    }
}

impl Guest for InProcess {
    fn ping(&mut self, value: i32) -> Result<i32> {
        self.ping.call(&mut self.store, value).ctx()
    }
    fn noop_loop(&mut self, count: i32) -> Result<i32> {
        self.noop_loop.call(&mut self.store, count).ctx()
    }
}

fn instance_pre(config: &wasmtime::Config, wasm: &Path) -> Result<wasmtime::InstancePre<()>> {
    let engine = wasmtime::Engine::new(config).ctx()?;
    let module = wasmtime::Module::from_file(&engine, wasm).ctx()?;
    let mut linker = wasmtime::Linker::<()>::new(&engine);
    linker
        .func_wrap("env", "HostNoop", |value: i32| value.wrapping_add(1))
        .ctx()?;
    linker.instantiate_pre(&module).ctx()
}

fn measure_instantiate(host: &str, metric: &str, pre: &wasmtime::InstancePre<()>) -> Result<()> {
    let mut samples = Vec::with_capacity(CREATE_RUNS);
    for i in 0..CREATE_RUNS as i32 {
        let start = Instant::now();
        let mut guest = InProcess::new(pre)?;
        guest.ping(i)?;
        samples.push(ns(start.elapsed()));
    }
    row(host, metric, samples);
    Ok(())
}

fn wasmtime_in_process(wasm: &Path) -> Result<()> {
    let host = "wasmtime in-process";
    let pre = instance_pre(&wasmtime::Config::new(), wasm)?;
    measure_instantiate(host, "instantiate + first call (on-demand)", &pre)?;

    let mut pooling = wasmtime::PoolingAllocationConfig::default();
    pooling.total_memories(16).total_tables(16).total_core_instances(16);
    let mut config = wasmtime::Config::new();
    config.allocation_strategy(wasmtime::InstanceAllocationStrategy::Pooling(pooling));
    measure_instantiate(host, "instantiate + first call (pooling)", &instance_pre(&config, wasm)?)?;

    measure_calls(host, &mut InProcess::new(&pre)?)
}

// ---------------------------------------------------------------------------------------------
// wasmtime, in a child process. Every host import crosses the pipe to the parent and back.

fn frame(op: u8, value: i32) -> [u8; 5] {
    let v = value.to_le_bytes();
    [op, v[0], v[1], v[2], v[3]]
}

struct Subprocess {
    child: Child,
    to_child: BufWriter<ChildStdin>,
    from_child: BufReader<ChildStdout>,
}

impl Subprocess {
    fn spawn(cwasm: &Path) -> Result<Self> {
        let mut child = Command::new(std::env::current_exe()?)
            .arg("child")
            .arg(cwasm)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let to_child = BufWriter::new(child.stdin.take().ok_or_else(|| anyhow!("no stdin"))?);
        let mut from_child = BufReader::new(child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?);
        let mut ready = [0u8; 1];
        from_child.read_exact(&mut ready)?;
        Ok(Self { child, to_child, from_child })
    }

    fn call(&mut self, op: u8, value: i32) -> Result<i32> {
        self.to_child.write_all(&frame(op, value))?;
        self.to_child.flush()?;
        loop {
            let mut buf = [0u8; 5];
            self.from_child.read_exact(&mut buf)?;
            let got = i32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]);
            match buf[0] {
                OP_RESULT => return Ok(got),
                OP_HOST_CALL => {
                    self.to_child.write_all(&got.wrapping_add(1).to_le_bytes())?;
                    self.to_child.flush()?;
                }
                other => bail!("unexpected frame {other}"),
            }
        }
    }
}

impl Drop for Subprocess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Guest for Subprocess {
    fn ping(&mut self, value: i32) -> Result<i32> {
        self.call(OP_PING, value)
    }
    fn noop_loop(&mut self, count: i32) -> Result<i32> {
        self.call(OP_LOOP, count)
    }
}

fn wasmtime_subprocess(wasm: &Path) -> Result<()> {
    let host = "wasmtime subprocess (pipes)";
    let engine = wasmtime::Engine::default();
    let module = wasmtime::Module::from_file(&engine, wasm).ctx()?;
    let cwasm = std::env::temp_dir().join("bench-guest.cwasm");
    std::fs::write(&cwasm, module.serialize().ctx()?)?;

    let mut spawn = Vec::with_capacity(SPAWN_RUNS);
    for i in 0..SPAWN_RUNS as i32 {
        let start = Instant::now();
        let mut guest = Subprocess::spawn(&cwasm)?;
        guest.ping(i)?;
        spawn.push(ns(start.elapsed()));
    }
    row(host, "spawn + instantiate + first call", spawn);

    measure_calls(host, &mut Subprocess::spawn(&cwasm)?)
}

struct ChildIo {
    from_parent: BufReader<std::io::Stdin>,
    to_parent: BufWriter<std::io::Stdout>,
}

/// Exits the child on a broken pipe: the parent has gone, so there is nobody to report to.
fn or_exit<T>(result: std::io::Result<T>) -> T {
    result.unwrap_or_else(|_| std::process::exit(2))
}

fn child(cwasm: &Path) -> Result<()> {
    let engine = wasmtime::Engine::default();
    // SAFETY: the parent wrote this file from `Module::serialize` on the same engine config.
    let module = unsafe { wasmtime::Module::deserialize_file(&engine, cwasm) }.ctx()?;
    let mut linker = wasmtime::Linker::<ChildIo>::new(&engine);
    linker
        .func_wrap(
            "env",
            "HostNoop",
            |mut caller: wasmtime::Caller<'_, ChildIo>, value: i32| -> i32 {
                let io = caller.data_mut();
                or_exit(io.to_parent.write_all(&frame(OP_HOST_CALL, value)));
                or_exit(io.to_parent.flush());
                let mut reply = [0u8; 4];
                or_exit(io.from_parent.read_exact(&mut reply));
                i32::from_le_bytes(reply)
            },
        )
        .ctx()?;
    let io = ChildIo {
        from_parent: BufReader::new(std::io::stdin()),
        to_parent: BufWriter::new(std::io::stdout()),
    };
    let mut store = wasmtime::Store::new(&engine, io);
    let instance = linker.instantiate(&mut store, &module).ctx()?;
    let ping = instance.get_typed_func::<i32, i32>(&mut store, "ping").ctx()?;
    let noop_loop = instance.get_typed_func::<i32, i32>(&mut store, "noop_loop").ctx()?;

    store.data_mut().to_parent.write_all(&[1])?;
    store.data_mut().to_parent.flush()?;
    loop {
        let mut request = [0u8; 5];
        if store.data_mut().from_parent.read_exact(&mut request).is_err() {
            return Ok(());
        }
        let value = i32::from_le_bytes([request[1], request[2], request[3], request[4]]);
        let result = match request[0] {
            OP_PING => ping.call(&mut store, value).ctx()?,
            OP_LOOP => noop_loop.call(&mut store, value).ctx()?,
            other => bail!("unexpected request {other}"),
        };
        store.data_mut().to_parent.write_all(&frame(OP_RESULT, result))?;
        store.data_mut().to_parent.flush()?;
    }
}

// ---------------------------------------------------------------------------------------------
// hyperlight-wasm

#[cfg(not(feature = "hyperlight"))]
fn hyperlight(_aot: Option<&Path>) -> Result<()> {
    eprintln!("hyperlight: skipped (built without the `hyperlight` feature)");
    Ok(())
}

#[cfg(feature = "hyperlight")]
fn hyperlight(aot: Option<&Path>) -> Result<()> {
    use std::sync::Arc;

    use hyperlight_wasm::{LoadedWasmSandbox, SandboxBuilder, WasmSandbox, is_hypervisor_present};

    struct Hyperlight(LoadedWasmSandbox);

    impl Guest for Hyperlight {
        fn ping(&mut self, value: i32) -> Result<i32> {
            self.0.call_guest_function::<i32>("ping", (value,)).ctx()
        }
        fn noop_loop(&mut self, count: i32) -> Result<i32> {
            self.0.call_guest_function::<i32>("noop_loop", (count,)).ctx()
        }
    }

    /// The trogon-sandbox-hyperlight-worker defaults: 256 KiB input buffer, 1.25 MiB heap.
    fn warm() -> Result<WasmSandbox> {
        let mut proto = SandboxBuilder::new()
            .with_guest_input_buffer_size(256 * 1024)
            .with_guest_heap_size(1280 * 1024)
            .build()
            .ctx()?;
        proto.register("HostNoop", |value: i32| value.wrapping_add(1)).ctx()?;
        proto.load_runtime().ctx()
    }

    let host = "hyperlight-wasm (KVM)";
    let aot = aot.ok_or_else(|| anyhow!("hyperlight needs the AOT-compiled guest"))?;
    if !is_hypervisor_present() {
        bail!("hyperlight: no hypervisor (/dev/kvm or /dev/mshv) on this host");
    }

    let mut cold = Vec::with_capacity(HL_CREATE_RUNS);
    let mut load = Vec::with_capacity(HL_CREATE_RUNS);
    for _ in 0..HL_CREATE_RUNS {
        let start = Instant::now();
        let sandbox = warm()?;
        cold.push(ns(start.elapsed()));
        let start = Instant::now();
        let mut guest = Hyperlight(sandbox.load_module(aot).ctx()?);
        guest.ping(1)?;
        load.push(ns(start.elapsed()));
    }
    row(host, "micro-VM build + runtime load", cold);
    row(host, "module load + first call (warm VM)", load);

    let mut guest = Hyperlight(warm()?.load_module(aot).ctx()?);
    let mut snap_times = Vec::with_capacity(100);
    let mut clean = guest.0.snapshot().ctx()?;
    for _ in 0..100 {
        let start = Instant::now();
        clean = guest.0.snapshot().ctx()?;
        snap_times.push(ns(start.elapsed()));
    }
    row(host, "snapshot", snap_times);

    let mut restore = Vec::with_capacity(HL_RESTORE_RUNS);
    let mut restore_call = Vec::with_capacity(HL_RESTORE_RUNS);
    for i in 0..HL_RESTORE_RUNS as i32 {
        guest.ping(i)?;
        let start = Instant::now();
        guest.0.restore(Arc::clone(&clean)).ctx()?;
        restore.push(ns(start.elapsed()));
        let start = Instant::now();
        guest.ping(i)?;
        restore_call.push(ns(start.elapsed()));
    }
    row(host, "restore to clean snapshot", restore);
    row(host, "first call after restore", restore_call);

    measure_calls(host, &mut guest)
}
