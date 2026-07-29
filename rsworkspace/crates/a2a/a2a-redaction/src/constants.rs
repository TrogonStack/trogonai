//! Crate-wide constants for a2a-redaction.

/// UTF-8 prefix written to guest linear memory when a skill refuses to redact a part.
///
/// Collisions with legitimate JSON payloads are bundle bugs; the gateway logs a warning.
pub const TIER3_REFUSE_SENTINEL: &[u8] = b"A2A_T3_REFUSE";

pub const SIGNED_BUNDLE_VERSION: u32 = 1;

/// Domain-separation tag the verifier and signer agree on. Including the tag
/// in the signed message prevents a confused-deputy attack where a signature
/// produced for some other Ed25519 message could be replayed here.
pub(crate) const SIGNED_BUNDLE_SIGNATURE_DOMAIN: &[u8] = b"a2a-redaction/signed-bundle/v1";

pub(crate) const SCRATCH_OFFSET: usize = 0x0800;
pub(crate) const GUEST_PAGE_BYTES: usize = 65536;
/// Cap on the length the guest can declare for its output buffer. Without a
/// ceiling, a malicious or buggy module can return a near-`i32::MAX` length
/// and force the host to allocate gigabytes (or OOM) before we'd even hit
/// the linear-memory read. We bound it to the single-page payload window
/// the guest is allowed to write into in the first place.
pub(crate) const MAX_GUEST_OUTPUT_BYTES: usize = GUEST_PAGE_BYTES;
/// Fuel budget for a single `redact_part` call. Wasmtime decrements this per
/// instruction executed; a guest that loops indefinitely traps with
/// `OutOfFuel` instead of blocking the caller thread. The value is sized
/// for the canonical per-part redact workload (one JSON part, scan-and-
/// replace); the gateway can lift the cap if a skill genuinely needs more.
pub(crate) const GUEST_FUEL_PER_CALL: u64 = 10_000_000;
/// Hard cap on a single store's linear-memory growth. The guest already
/// only writes into one page worth of scratch, but a buggy module could
/// allocate more pages internally; bound that to keep one bad guest from
/// pinning the host's RAM.
pub(crate) const MAX_STORE_MEMORY_BYTES: usize = 16 * 1024 * 1024;
