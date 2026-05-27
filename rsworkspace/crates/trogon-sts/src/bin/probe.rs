use std::collections::VecDeque;
use std::time::{Duration, Instant};

use async_nats::ConnectOptions;
use clap::Parser;
use serde_json::json;
use tracing::{info, warn};

use trogon_sts::types::StsExchangeRequest;
use trogon_sts::EXCHANGE_SUBJECT;

const METRICS_SUBJECT: &str = "mcp.metrics.sts.latency";
const VIOLATION_SUBJECT: &str = "mcp.audit.sts.latency_violation";

#[derive(Debug, Parser)]
#[command(name = "trogon-sts-probe", about = "Synthetic STS exchange latency probe sidecar")]
struct Args {
    #[arg(long, env = "NATS_URL")]
    nats_url: String,

    #[arg(long, env = "MCP_STS_PROBE_SUBJECT_TOKEN")]
    subject_token: String,

    #[arg(long, env = "MCP_STS_PROBE_ACTOR_TOKEN")]
    actor_token: String,

    #[arg(long, env = "MCP_STS_PROBE_AUDIENCE")]
    audience: String,

    #[arg(long, default_value = "oncall-probe", env = "MCP_STS_PROBE_PURPOSE")]
    purpose: String,

    #[arg(long, default_value = "10", env = "MCP_STS_PROBE_INTERVAL_SECS")]
    probe_interval_secs: u64,

    #[arg(long, default_value = "40", env = "MCP_STS_PROBE_P99_THRESHOLD_MS")]
    p99_threshold_ms: u64,

    #[arg(long, default_value = "60", env = "MCP_STS_PROBE_WINDOW_SECS")]
    window_secs: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args = Args::parse();
    let nats = ConnectOptions::new().connect(&args.nats_url).await?;
    info!(
        interval_secs = args.probe_interval_secs,
        threshold_ms = args.p99_threshold_ms,
        "trogon-sts-probe started"
    );

    let mut samples: VecDeque<(Instant, u64)> = VecDeque::new();
    let window = Duration::from_secs(args.window_secs);
    let interval = Duration::from_secs(args.probe_interval_secs);

    loop {
        let started = Instant::now();
        let request = StsExchangeRequest {
            subject_token: args.subject_token.clone(),
            subject_token_type: "urn:ietf:params:oauth:token-type:jwt".into(),
            actor_token: args.actor_token.clone(),
            audience: args.audience.clone(),
            scope: String::new(),
            purpose: args.purpose.clone(),
            requested_token_type: "urn:ietf:params:oauth:token-type:jwt".into(),
        };

        let payload = serde_json::to_vec(&request)?;
        match nats.request(EXCHANGE_SUBJECT, payload.into()).await {
            Ok(_reply) => {
                let latency_ms = started.elapsed().as_millis() as u64;
                let now = Instant::now();
                samples.push_back((now, latency_ms));
                while let Some((ts, _)) = samples.front()
                    && now.duration_since(*ts) > window
                {
                    samples.pop_front();
                }

                let (p50, p95, p99) = percentiles(&samples);
                let metric = json!({
                    "schema": "trogon.mcp.metrics.sts.latency/v1",
                    "p50_ms": p50,
                    "p95_ms": p95,
                    "p99_ms": p99,
                    "sample_count": samples.len(),
                });
                let _ = nats
                    .publish(METRICS_SUBJECT, serde_json::to_vec(&metric)?.into())
                    .await;

                if p99 > args.p99_threshold_ms {
                    warn!(p99_ms = p99, threshold_ms = args.p99_threshold_ms, "p99 latency threshold exceeded");
                    let violation = json!({
                        "schema": "trogon.mcp.audit.sts.latency_violation/v1",
                        "p99_ms": p99,
                        "threshold_ms": args.p99_threshold_ms,
                        "p50_ms": p50,
                        "p95_ms": p95,
                    });
                    let _ = nats
                        .publish(VIOLATION_SUBJECT, serde_json::to_vec(&violation)?.into())
                        .await;
                }
            }
            Err(err) => {
                warn!(error = %err, "synthetic exchange failed");
            }
        }

        tokio::time::sleep(interval).await;
    }
}

fn percentiles(samples: &VecDeque<(Instant, u64)>) -> (u64, u64, u64) {
    if samples.is_empty() {
        return (0, 0, 0);
    }
    let mut values: Vec<u64> = samples.iter().map(|(_, ms)| *ms).collect();
    values.sort_unstable();
    let p50 = values[values.len() * 50 / 100];
    let p95 = values[values.len() * 95 / 100];
    let p99 = values[values.len().saturating_sub(1).max(values.len() * 99 / 100)];
    (p50, p95, p99)
}
