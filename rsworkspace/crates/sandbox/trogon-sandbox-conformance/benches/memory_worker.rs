//! Lease and command costs against the in-process memory worker over a NATS server in a container, so the figures are
//! the platform's own overhead: NATS round trips, JSON encoding and chunked transfer, with no container or VM behind
//! the lease. Needs Docker.
//!
//! The figures are for comparing two builds on the same machine, not for gating CI.
//!
//! ```text
//! cargo bench -p trogon-sandbox-conformance --bench memory_worker
//! ```

use std::error::Error;
use std::future::Future;
use std::io::Write;
use std::time::{Duration, Instant};

use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_conformance::{BenchLease, CheckFailureError, Target, percentile};
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_memory_worker::{MemoryWorker, MemoryWorkerConfig};

/// Untimed runs before each measurement.
const WARMUP: usize = 5;

/// Timed runs per measurement.
const SAMPLES: usize = 50;

/// The file each `cat` reads back.
const OUTPUT_PATH: &str = "/work/out.bin";

/// Sizes of the command output measured.
const OUTPUT_SIZES: [usize; 3] = [1024, 64 * 1024, 512 * 1024];

/// The p50 and p95 of [`SAMPLES`] runs of `run`, after [`WARMUP`] untimed ones.
async fn time<F, Fut>(mut run: F) -> Result<(Duration, Duration), CheckFailureError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<(), CheckFailureError>>,
{
    for _ in 0..WARMUP {
        run().await?;
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let start = Instant::now();
        run().await?;
        samples.push(start.elapsed());
    }
    samples.sort();
    Ok((
        percentile(&samples, 50.0).unwrap_or_default(),
        percentile(&samples, 95.0).unwrap_or_default(),
    ))
}

fn row(name: &str, (p50, p95): (Duration, Duration), throughput: Option<usize>) -> std::io::Result<()> {
    let rate = throughput
        .map(|bytes| {
            let mib = bytes as f64 / (1024.0 * 1024.0);
            format!("{:>10.1} MiB/s", mib / p50.as_secs_f64().max(f64::EPSILON))
        })
        .unwrap_or_default();
    writeln!(
        std::io::stdout(),
        "{name:<40} p50 {:>9.3} ms  p95 {:>9.3} ms {rate}",
        p50.as_secs_f64() * 1e3,
        p95.as_secs_f64() * 1e3
    )
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // `cargo bench` passes `--bench`; a filter argument is not supported, so every measurement runs.
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let worker = MemoryWorker::start(client.clone(), MemoryWorkerConfig::default()).await?;
    let mut target = Target::new(client, ProviderKind::Mock);
    target.timeout = Duration::from_secs(10);

    let round_trip = time(|| async {
        let lease = BenchLease::acquire(&target).await?;
        lease.release().await;
        Ok(())
    })
    .await?;
    row("lease create and release round trip", round_trip, None)?;

    for len in OUTPUT_SIZES {
        let lease = BenchLease::acquire(&target).await?;
        lease.seed_file(OUTPUT_PATH, len).await?;
        let lease_ref = &lease;
        let cat = time(|| async move {
            let read = lease_ref.cat(OUTPUT_PATH).await?;
            if read == len {
                Ok(())
            } else {
                Err(CheckFailureError::Mismatch {
                    expected: format!("{len} bytes of output"),
                    observed: format!("{read} bytes"),
                })
            }
        })
        .await;
        lease.release().await;
        row(&format!("command output: cat {len} bytes"), cat?, Some(len))?;
    }

    worker.shutdown().await;
    Ok(())
}
