/// The queue group every Hyperlight worker joins, so one worker takes each dispatch.
pub(crate) const QUEUE_GROUP: &str = "sandbox-provider-hyperlight";

/// Bytes of input buffer each guest gets by default: what the worker hands a guest per call.
pub(crate) const DEFAULT_GUEST_INPUT_BUFFER_BYTES: usize = 256 * 1024;

/// Bytes of output buffer each guest gets by default: what a guest hands back, as a return value
/// or host-call argument. Hyperlight's own default is 16 KiB, and a guest handing back more
/// aborts, so this matches the input buffer instead.
pub(crate) const DEFAULT_GUEST_OUTPUT_BUFFER_BYTES: usize = DEFAULT_GUEST_INPUT_BUFFER_BYTES;

/// Bytes of heap each guest gets by default. The guest's Wasm linear memory lives in it.
pub(crate) const DEFAULT_GUEST_HEAP_BYTES: u64 = 1280 * 1024;

/// Media type of the single layer of a Hyperlight snapshot artifact.
pub const HYPERLIGHT_SNAPSHOT_MEDIA_TYPE: &str = "application/vnd.trogon.hyperlight.snapshot.v1";
