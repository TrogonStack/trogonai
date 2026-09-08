use std::time::Duration;

/// Default fuel budget for a single guest export call (`decide`, `evolve`, or
/// `snapshot`). Wasmtime decrements fuel per instruction executed, so a guest
/// stuck in an infinite loop traps with `OutOfFuel` instead of blocking the
/// host indefinitely.
pub const DEFAULT_FUEL_PER_CALL: u64 = 10_000_000;

/// Default ceiling on a single store's linear-memory growth. Bounds how much
/// memory one guest session can pin regardless of how many events it replays.
pub const DEFAULT_MAX_MEMORY_BYTES: usize = 64 * 1024 * 1024;

/// Default ceiling on the element count of any table a guest session's store
/// may create.
///
/// Wasmtime otherwise leaves `table.grow` unbounded. Inspecting the compiled
/// schedules decider component with `wasm-tools print` shows its largest
/// table (the guest module's indirect-call table) is fixed at 270 elements;
/// this ceiling leaves generous headroom for larger deciders while still
/// rejecting a guest that tries to grow a table without bound.
pub const DEFAULT_MAX_TABLE_ELEMENTS: usize = 8_192;

/// Default ceiling on how many core wasm module instantiations a single
/// guest session's store may create.
///
/// Every component this crate loads implements the fixed
/// `trogon:decider@0.3.0` world, which declares exactly one resource type
/// (`session`). Inspecting the compiled schedules decider component with
/// `wasm-tools print` shows that shape always instantiates 3 real core
/// modules: the guest's own module plus a pair of tiny resource-destructor
/// shim modules `cargo component` emits to break a circular dependency
/// between the guest module and its destructor. That structure is invariant
/// across every decider this crate loads. This ceiling leaves headroom above
/// the measured minimum of 3 without reopening wasmtime's own default of
/// 10,000.
pub const DEFAULT_MAX_INSTANCES_PER_SESSION: usize = 8;

/// Default ceiling on how many wasm tables a single guest session's store may
/// create. The schedules decider fixture allocates 2 (the main module's
/// indirect-call table and the destructor shim's 1-element table); see
/// [`DEFAULT_MAX_INSTANCES_PER_SESSION`] for how this was measured.
pub const DEFAULT_MAX_TABLES_PER_SESSION: usize = 6;

/// Default ceiling on how many wasm linear memories a single guest session's
/// store may create. The schedules decider fixture allocates exactly 1 (only
/// the main module defines a memory; the destructor shims do not); see
/// [`DEFAULT_MAX_INSTANCES_PER_SESSION`] for how this was measured.
pub const DEFAULT_MAX_MEMORIES_PER_SESSION: usize = 4;

/// Default ceiling on how many guest sessions the pooling allocator keeps
/// warm allocation slots for concurrently.
///
/// The pooling allocator's aggregate totals are sized as this many sessions'
/// worth of [`DEFAULT_MAX_INSTANCES_PER_SESSION`] core instances,
/// [`DEFAULT_MAX_TABLES_PER_SESSION`] tables, and
/// [`DEFAULT_MAX_MEMORIES_PER_SESSION`] memories.
pub const DEFAULT_MAX_CONCURRENT_SESSIONS: usize = 256;

/// Default cadence at which the engine's background ticker increments
/// wasmtime's epoch counter for epoch-based interruption.
pub const DEFAULT_EPOCH_TICK_INTERVAL: Duration = Duration::from_millis(50);

/// Default number of epoch ticks a single guest export call is allowed
/// before it is interrupted. At the default [`DEFAULT_EPOCH_TICK_INTERVAL`]
/// cadence this is a 2 second wall-clock budget per call.
pub const DEFAULT_EPOCH_TICKS_PER_CALL: u64 = 40;

pub(crate) const METER_NAME: &str = "trogon-decider-wasm-runtime";
