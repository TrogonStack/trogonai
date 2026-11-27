//! Boundary cost of one guest on three hosts: wasmtime in process, wasmtime in a child process
//! whose host imports are served over pipes, and hyperlight-wasm in a KVM/MSHV micro-VM.
//!
//! Usage: `bench-host all <guest.wasm> [<guest.aot>]` prints Markdown table rows; the AOT guest is
//! required when built with the `hyperlight` feature. `bench-host child <guest.cwasm>` is the
//! subprocess side and is not meant to be run by hand.

use std::io::{BufWriter, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Instant;

use anyhow::{Result, anyhow, bail};

const WARMUP_CALLS: usize = 1_000;
const PING_CALLS: usize = 10_000;
const LOOP_COUNT: i32 = 10_000;
const LOOP_RUNS: usize = 50;
const CREATE_RUNS: usize = 100;
const SPAWN_RUNS: usize = 30;
const HL_CREATE_RUNS: usize = 30;
const HL_SNAPSHOT_RUNS: usize = 100;
const HL_RESTORE_RUNS: usize = 1_000;
const PAYLOAD_SIZES: [i32; 2] = [1024, 64 * 1024];
const PAYLOAD_CALLS: usize = 1_000;

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

/// The hosts that get payload rows.
trait PayloadGuest: Guest {
    /// Writes `message` into guest memory and has the guest `consume` it.
    fn consume(&mut self, message: Vec<u8>) -> Result<i32>;
    /// Has the guest send a `len`-byte buffer to the host `count` times.
    fn bytes_loop(&mut self, count: i32, len: i32) -> Result<i32>;
    /// Returns the guest to a clean state, so a size that broke it cannot fail the next series.
    fn reset(&mut self) -> Result<()>;
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("child") => child(Path::new(arg(&args, 2)?)),
        Some("all") => {
            let wasm = Path::new(arg(&args, 2)?);
            timer_floor();
            wasmtime_in_process(wasm)?;
            wasmtime_subprocess(wasm)?;
            hyperlight(&args)
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
// Timing and reporting

/// Runs `f` and returns its output with the time it took, in nanoseconds.
fn timed<T>(f: impl FnOnce() -> Result<T>) -> Result<(T, f64)> {
    let start = Instant::now();
    let out = f()?;
    Ok((out, start.elapsed().as_nanos() as f64))
}

/// Times `runs` calls of `f`, passing each its run index. Whatever `f` returns is dropped after
/// the clock stops, so teardown stays out of the sample.
fn sample<T>(runs: usize, mut f: impl FnMut(i32) -> Result<T>) -> Result<Vec<f64>> {
    (0..runs as i32).map(|i| Ok(timed(|| f(i))?.1)).collect()
}

/// Nearest-rank percentile, as `trogon-sandbox-conformance`'s latency report computes it.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn row(host: &str, metric: &str, mut samples: Vec<f64>) {
    samples.sort_by(f64::total_cmp);
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    println!(
        "| {host} | {metric} | {} | {} | {} | {} |",
        samples.len(),
        fmt(percentile(&samples, 50.0)),
        fmt(percentile(&samples, 99.0)),
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

/// What an empty `Instant` pair costs: the floor under every single-call sample.
fn timer_floor() {
    let samples = (0..PING_CALLS)
        .map(|_| {
            let start = Instant::now();
            start.elapsed().as_nanos() as f64
        })
        .collect();
    row("—", "timer overhead (empty `Instant` pair)", samples);
}

/// Times `PING_CALLS` single host-to-guest calls, then `LOOP_RUNS` batches of `LOOP_COUNT`
/// guest-to-host calls, reporting each batch as the cost of one call.
fn measure_calls(host: &str, guest: &mut impl Guest) -> Result<()> {
    for i in 0..WARMUP_CALLS as i32 {
        guest.ping(i)?;
    }
    guest.noop_loop(LOOP_COUNT)?;

    let ping = sample(PING_CALLS, |i| match guest.ping(i)? {
        got if got == i => Ok(()),
        got => bail!("{host}: ping({i}) returned {got}"),
    })?;
    row(host, "host→guest call (`ping`)", ping);

    let batches = sample(LOOP_RUNS, |_| match guest.noop_loop(LOOP_COUNT)? {
        LOOP_COUNT => Ok(()),
        got => bail!("{host}: noop_loop returned {got}"),
    })?;
    let per_call = batches.into_iter().map(|t| t / f64::from(LOOP_COUNT)).collect();
    row(host, &format!("guest→host call (amortized over {LOOP_COUNT})"), per_call);
    Ok(())
}

/// Like `measure_calls`, with a payload of each `PAYLOAD_SIZES` size. A call the host refuses
/// becomes a "failed" row instead of ending the run, since a refused size is itself a result; a
/// wrong answer still panics, since that is a bug in the bench.
fn measure_payloads(host: &str, guest: &mut impl PayloadGuest) -> Result<()> {
    let count = PAYLOAD_CALLS as i32;
    for len in PAYLOAD_SIZES {
        let kib = len / 1024;
        let message = vec![7u8; len as usize];

        guest.reset()?;
        // Each message is cloned just before its call, like a freshly serialized one: cache-hot,
        // and its allocation outside the clock.
        let into = guest.consume(message.clone()).and_then(|_| {
            (0..PAYLOAD_CALLS)
                .map(|_| {
                    let next = message.clone();
                    let (got, took) = timed(|| guest.consume(next))?;
                    assert_eq!(got, len, "{host}: consume");
                    Ok(took)
                })
                .collect::<Result<Vec<f64>>>()
        });
        report(host, &format!("host→guest call with {kib} KiB"), into);

        guest.reset()?;
        let out = guest.bytes_loop(count, len).and_then(|_| {
            sample(LOOP_RUNS, |_| {
                assert_eq!(guest.bytes_loop(count, len)?, count, "{host}: bytes_loop");
                Ok(())
            })
        });
        let out = out.map(|batches| batches.into_iter().map(|t| t / f64::from(count)).collect());
        report(host, &format!("guest→host call with {kib} KiB (amortized over {count})"), out);
    }
    Ok(())
}

/// A row for `samples`, or a "failed" row with the error.
fn report(host: &str, metric: &str, samples: Result<Vec<f64>>) {
    match samples {
        Ok(samples) => row(host, metric, samples),
        Err(error) => {
            let error = format!("{error:#}").replace(['|', '\n'], " ");
            println!("| {host} | {metric} | — | failed: {error:.160} | | |");
        }
    }
}

/// In-process store data: the guest's memory, kept so `host_bytes` skips an export lookup.
type Data = Option<wasmtime::Memory>;

/// Copies `len` bytes at `ptr` out of the caller's memory, as Hyperlight's marshalling does,
/// and returns how many arrived (-1 when the range is out of bounds).
fn host_bytes(caller: wasmtime::Caller<'_, Data>, ptr: i32, len: i32) -> i32 {
    let Some(memory) = *caller.data() else {
        return -1;
    };
    memory
        .data(&caller)
        .get(ptr as usize..)
        .and_then(|data| data.get(..len as usize))
        .map_or(-1, |bytes| std::hint::black_box(bytes.to_vec()).len() as i32)
}

// ---------------------------------------------------------------------------------------------
// wasmtime, in process

struct InProcess {
    store: wasmtime::Store<Data>,
    memory: wasmtime::Memory,
    ping: wasmtime::TypedFunc<i32, i32>,
    noop_loop: wasmtime::TypedFunc<i32, i32>,
    malloc: wasmtime::TypedFunc<i32, i32>,
    consume: wasmtime::TypedFunc<(i32, i32), i32>,
    bytes_loop: wasmtime::TypedFunc<(i32, i32), i32>,
}

impl InProcess {
    fn new(pre: &wasmtime::InstancePre<Data>) -> Result<Self> {
        let mut store = wasmtime::Store::new(pre.module().engine(), None);
        let instance = pre.instantiate(&mut store).ctx()?;
        let ping = instance.get_typed_func(&mut store, "ping").ctx()?;
        let noop_loop = instance.get_typed_func(&mut store, "noop_loop").ctx()?;
        let malloc = instance.get_typed_func(&mut store, "malloc").ctx()?;
        let consume = instance.get_typed_func(&mut store, "consume").ctx()?;
        let bytes_loop = instance.get_typed_func(&mut store, "bytes_loop").ctx()?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| anyhow!("guest exports no memory"))?;
        *store.data_mut() = Some(memory);
        Ok(Self {
            store,
            memory,
            ping,
            noop_loop,
            malloc,
            consume,
            bytes_loop,
        })
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

impl PayloadGuest for InProcess {
    fn consume(&mut self, message: Vec<u8>) -> Result<i32> {
        let len = message.len() as i32;
        let ptr = self.malloc.call(&mut self.store, len).ctx()?;
        self.memory.write(&mut self.store, ptr as usize, &message).ctx()?;
        self.consume.call(&mut self.store, (ptr, len)).ctx()
    }
    fn bytes_loop(&mut self, count: i32, len: i32) -> Result<i32> {
        self.bytes_loop.call(&mut self.store, (count, len)).ctx()
    }
    /// Each call already starts from the guest's own state; nothing to restore.
    fn reset(&mut self) -> Result<()> {
        Ok(())
    }
}

fn instance_pre(config: &wasmtime::Config, wasm: &Path) -> Result<wasmtime::InstancePre<Data>> {
    let engine = wasmtime::Engine::new(config).ctx()?;
    let module = wasmtime::Module::from_file(&engine, wasm).ctx()?;
    let mut linker = wasmtime::Linker::<Data>::new(&engine);
    linker
        .func_wrap("env", "HostNoop", |value: i32| value.wrapping_add(1))
        .ctx()?;
    linker.func_wrap("env", "HostBytes", host_bytes).ctx()?;
    linker.instantiate_pre(&module).ctx()
}

fn measure_instantiate(host: &str, metric: &str, pre: &wasmtime::InstancePre<Data>) -> Result<()> {
    let samples = sample(CREATE_RUNS, |i| {
        let mut guest = InProcess::new(pre)?;
        guest.ping(i)?;
        Ok(guest)
    })?;
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

    let mut guest = InProcess::new(&pre)?;
    measure_calls(host, &mut guest)?;
    measure_payloads(host, &mut guest)
}

// ---------------------------------------------------------------------------------------------
// wasmtime, in a child process. Every message, host import calls included, is one 5-byte frame
// over a pipe: an op byte and a little-endian i32.

fn send(to: &mut impl Write, op: u8, value: i32) -> std::io::Result<()> {
    let v = value.to_le_bytes();
    to.write_all(&[op, v[0], v[1], v[2], v[3]])?;
    to.flush()
}

fn receive(from: &mut impl Read) -> std::io::Result<(u8, i32)> {
    let mut b = [0u8; 5];
    from.read_exact(&mut b)?;
    Ok((b[0], i32::from_le_bytes([b[1], b[2], b[3], b[4]])))
}

struct Subprocess {
    child: Child,
    to_child: BufWriter<ChildStdin>,
    from_child: ChildStdout,
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
        let mut from_child = child.stdout.take().ok_or_else(|| anyhow!("no stdout"))?;
        receive(&mut from_child)?;
        Ok(Self { child, to_child, from_child })
    }

    fn call(&mut self, op: u8, value: i32) -> Result<i32> {
        send(&mut self.to_child, op, value)?;
        loop {
            match receive(&mut self.from_child)? {
                (OP_RESULT, got) => return Ok(got),
                (OP_HOST_CALL, got) => send(&mut self.to_child, OP_RESULT, got.wrapping_add(1))?,
                (other, _) => bail!("unexpected frame {other}"),
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

    let spawn = sample(SPAWN_RUNS, |i| {
        let mut guest = Subprocess::spawn(&cwasm)?;
        guest.ping(i)?;
        Ok(guest)
    })?;
    row(host, "spawn + instantiate + first call", spawn);

    measure_calls(host, &mut Subprocess::spawn(&cwasm)?)
}

struct ChildIo {
    from_parent: std::io::StdinLock<'static>,
    to_parent: BufWriter<std::io::StdoutLock<'static>>,
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
                send(&mut io.to_parent, OP_HOST_CALL, value)
                    .and_then(|()| receive(&mut io.from_parent))
                    // A broken pipe means the parent is gone: nobody is left to report to.
                    .unwrap_or_else(|_| std::process::exit(2))
                    .1
            },
        )
        .ctx()?;
    // Imported by the guest but only exercised by the payload rows, which this host skips.
    linker.func_wrap("env", "HostBytes", |_: i32, _: i32| -1).ctx()?;
    let io = ChildIo {
        from_parent: std::io::stdin().lock(),
        to_parent: BufWriter::new(std::io::stdout().lock()),
    };
    let mut store = wasmtime::Store::new(&engine, io);
    let instance = linker.instantiate(&mut store, &module).ctx()?;
    let ping = instance.get_typed_func::<i32, i32>(&mut store, "ping").ctx()?;
    let noop_loop = instance.get_typed_func::<i32, i32>(&mut store, "noop_loop").ctx()?;

    send(&mut store.data_mut().to_parent, OP_RESULT, 0)?;
    while let Ok((op, value)) = receive(&mut store.data_mut().from_parent) {
        let result = match op {
            OP_PING => ping.call(&mut store, value).ctx()?,
            OP_LOOP => noop_loop.call(&mut store, value).ctx()?,
            other => bail!("unexpected request {other}"),
        };
        send(&mut store.data_mut().to_parent, OP_RESULT, result)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// hyperlight-wasm

#[cfg(not(feature = "hyperlight"))]
fn hyperlight(_args: &[String]) -> Result<()> {
    eprintln!("hyperlight: skipped (built without the `hyperlight` feature)");
    Ok(())
}

#[cfg(feature = "hyperlight")]
fn hyperlight(args: &[String]) -> Result<()> {
    use std::sync::Arc;

    use hyperlight_wasm::{
        LoadedWasmSandbox, SandboxBuilder, Snapshot, WasmSandbox, is_hypervisor_present,
    };

    /// A loaded sandbox and, once taken, the clean snapshot `reset` rewinds to.
    struct Hyperlight(LoadedWasmSandbox, Option<Arc<Snapshot>>);

    impl Guest for Hyperlight {
        fn ping(&mut self, value: i32) -> Result<i32> {
            self.0.call_guest_function::<i32>("ping", (value,)).ctx()
        }
        fn noop_loop(&mut self, count: i32) -> Result<i32> {
            self.0.call_guest_function::<i32>("noop_loop", (count,)).ctx()
        }
    }

    impl PayloadGuest for Hyperlight {
        fn consume(&mut self, message: Vec<u8>) -> Result<i32> {
            let len = message.len() as i32;
            self.0.call_guest_function::<i32>("consume", (message, len)).ctx()
        }
        fn bytes_loop(&mut self, count: i32, len: i32) -> Result<i32> {
            self.0.call_guest_function::<i32>("bytes_loop", (count, len)).ctx()
        }
        fn reset(&mut self) -> Result<()> {
            match &self.1 {
                Some(clean) => self.0.restore(Arc::clone(clean)).ctx(),
                None => Ok(()),
            }
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
        proto
            .register("HostBytes", |data: Vec<u8>, len: i32| {
                if data.len() == len as usize { len } else { -1 }
            })
            .ctx()?;
        proto.load_runtime().ctx()
    }

    let host = "hyperlight-wasm (KVM)";
    let aot = Path::new(arg(args, 3)?);
    if !is_hypervisor_present() {
        bail!("hyperlight: no hypervisor (/dev/kvm or /dev/mshv) on this host");
    }

    let mut cold = Vec::with_capacity(HL_CREATE_RUNS);
    let mut load = Vec::with_capacity(HL_CREATE_RUNS);
    for _ in 0..HL_CREATE_RUNS {
        let (sandbox, built) = timed(warm)?;
        let (_guest, loaded) = timed(|| {
            let mut guest = Hyperlight(sandbox.load_module(aot).ctx()?, None);
            guest.ping(1)?;
            Ok(guest)
        })?;
        cold.push(built);
        load.push(loaded);
    }
    row(host, "micro-VM build + runtime load", cold);
    row(host, "module load + first call (warm VM)", load);

    let mut guest = Hyperlight(warm()?.load_module(aot).ctx()?, None);
    let mut snapshots = Vec::with_capacity(HL_SNAPSHOT_RUNS);
    let mut clean = None;
    for _ in 0..HL_SNAPSHOT_RUNS {
        let (snapshot, took) = timed(|| guest.0.snapshot().ctx())?;
        snapshots.push(took);
        // Replacing frees the previous snapshot here, after the clock stopped.
        clean = Some(snapshot);
    }
    let clean = clean.ok_or_else(|| anyhow!("no snapshot"))?;
    row(host, "snapshot", snapshots);

    let mut restore = Vec::with_capacity(HL_RESTORE_RUNS);
    let mut first_call = Vec::with_capacity(HL_RESTORE_RUNS);
    for i in 0..HL_RESTORE_RUNS as i32 {
        guest.ping(i)?;
        restore.push(timed(|| guest.0.restore(Arc::clone(&clean)).ctx())?.1);
        first_call.push(timed(|| guest.ping(i))?.1);
    }
    row(host, "restore to clean snapshot", restore);
    row(host, "first call after restore", first_call);
    guest.1 = Some(clean);

    measure_calls(host, &mut guest)?;
    measure_payloads(host, &mut guest)
}
