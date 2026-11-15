/// Rows per `POST /worker/events` (`maxBatchSize`).
pub const MAX_BATCH_EVENTS: usize = 100;

/// Rows the queue holds before a writer has to wait (`maxQueueSize`).
pub const MAX_QUEUE_EVENTS: usize = 100_000;

/// One mebibyte (`hm`).
pub(super) const MIB: usize = 1_048_576;

/// Bytes per batch: `10 * hm`, with no worker socket to lower it.
pub const MAX_BATCH_BYTES: usize = 10 * MIB;

/// The batch size from which a request with no answer is retried in halves (`iC`).
pub const SHRINK_FROM_BYTES: usize = 262_144;

/// The request timeout floor (`sC`).
pub const POST_TIMEOUT_FLOOR_MS: u64 = 10_000;

/// The request timeout ceiling (`q2e`, `maxEventPostTimeoutMs`).
pub const POST_TIMEOUT_CEILING_MS: u64 = 120_000;

/// The extra seconds per mebibyte a request is given (`V2e` / 1000).
pub(super) const SECONDS_PER_MIB: f64 = 5.0;

/// How much a failed large request widens its next timeout (`Y2e`).
pub(super) const TIMEOUT_GROWTH: f64 = 1.5;

pub(super) const BASE_DELAY_MS: f64 = 500.0;

pub(super) const MAX_DELAY_MS: f64 = 30_000.0;

pub(super) const JITTER_MS: f64 = 500.0;
