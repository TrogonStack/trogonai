#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! Each scenario runs in a child process because the providers are
//! process-global and the OTLP endpoint is read from the environment.

use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

use trogon_std::env::{InMemoryEnv, ReadEnv, SystemEnv};
use trogon_std::fs::MemFs;
use trogon_telemetry::constants::SHUTDOWN_TIMEOUT;
use trogon_telemetry::{ServiceName, init_logger, meter, shutdown_otel};

const ENDPOINT_VARIABLE: &str = "TROGON_TELEMETRY_PROBE_ENDPOINT";
const MARKER: &str = "SHUTDOWN_MILLIS=";

fn shutdown_millis_against(endpoint: &str) -> u128 {
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_emits_telemetry_and_times_shutdown", "--nocapture"])
        .env(ENDPOINT_VARIABLE, endpoint)
        .env("OTEL_EXPORTER_OTLP_ENDPOINT", endpoint)
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find_map(|line| line.strip_prefix(MARKER))
        .unwrap_or_else(|| panic!("child reported no shutdown time: {stdout}"));
    line.parse().unwrap()
}

#[test]
#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(
        telemetry_span_name_literal,
        telemetry_metric_construction,
        telemetry_metric_name_literal,
        reason = "the probe emits throwaway telemetry that no semantic convention describes"
    )
)]
fn child_emits_telemetry_and_times_shutdown() {
    if SystemEnv.var(ENDPOINT_VARIABLE).is_err() {
        return;
    }
    init_logger(ServiceName::TrogonEgressProxy, [], &InMemoryEnv::new(), &MemFs::new());
    {
        let span = tracing::info_span!("probe_span");
        let _entered = span.enter();
        tracing::info!("probe log");
    }
    meter("probe").u64_counter("probe_total").build().add(1, &[]);
    let started = Instant::now();
    let _ = shutdown_otel();
    println!("{MARKER}{}", started.elapsed().as_millis());
}

#[test]
fn shutdown_is_bounded_when_the_collector_accepts_connections_but_never_answers() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let held = std::thread::spawn(move || {
        let mut connections = Vec::new();
        while let Ok((stream, _)) = listener.accept() {
            connections.push(stream);
        }
    });
    let elapsed = Duration::from_millis(shutdown_millis_against(&endpoint) as u64);
    assert!(elapsed < SHUTDOWN_TIMEOUT + Duration::from_secs(1), "{elapsed:?}");
    drop(held);
}

#[test]
fn shutdown_is_bounded_when_the_collector_port_is_closed() {
    let elapsed = Duration::from_millis(shutdown_millis_against("http://127.0.0.1:1") as u64);
    assert!(elapsed < SHUTDOWN_TIMEOUT + Duration::from_secs(1), "{elapsed:?}");
}
