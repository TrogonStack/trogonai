//! Runs the conformance suite against the memory worker and, on request, against workers that are already running.
//!
//! The memory worker runs in process on a NATS server in a container in a normal `cargo test`. The docker and
//! subprocess workers need their real runtime, so those tests are ignored and expect a worker that is already running
//! against a NATS server you point them at:
//!
//! ```text
//! # terminal 1: a NATS server
//! nats-server -p 4222
//! # terminal 2: the worker under test (docker shown; the subprocess worker runs on Linux and macOS)
//! cargo run -p trogon-sandbox-docker-worker -- --nats-url nats://127.0.0.1:4222 \
//!   --egress-proxy-image trogon-sandbox-egress-proxyd:dev --owner conformance
//! # terminal 3: the suite
//! SANDBOX_CONFORMANCE_NATS_URL=nats://127.0.0.1:4222 \
//!   cargo test -p trogon-sandbox-conformance --test conformance -- --ignored local_docker
//! ```
//!
//! `SANDBOX_CONFORMANCE_IMAGE` overrides the image leased from image-based workers (default `alpine:3.20`). The
//! docker worker execs into the container the image starts, so the image must keep running: plain `alpine` exits at
//! once. Build one that does:
//!
//! ```text
//! printf 'FROM alpine:3.20\nCMD ["sleep","infinity"]\n' | docker build -t sandbox-conformance:local -
//! SANDBOX_CONFORMANCE_IMAGE=sandbox-conformance:local ...
//! ```
//!
//! A worker started with `SANDBOX_DISPATCH_VERIFY=require` refuses unsigned requests. Export the broker's signing key
//! variables (`SANDBOX_DISPATCH_SIGNING_KEY_FILE` or `SANDBOX_DISPATCH_SIGNING_KEY`) to the suite and it signs every
//! dispatch and per-lease request; the worker needs the matching `SANDBOX_DISPATCH_PUBLIC_KEYS`.
//! `tests/signed_dispatch.rs` proves this against the memory worker in `require` mode, and `tests/entity.rs` covers
//! the broker-side entity modes through an in-process broker.
//!
//! The subprocess worker serves no `run_command`: a lease is the agent process it launches. Its target declares
//! [`ExecModel::None`], so only the lease lifecycle checks and the file verb checks (served from the host side of
//! `/workspace`) run against it.

mod support;

use std::error::Error;
use std::time::Duration;

use support::memory::{drained, external_target, memory_target};
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_conformance::{
    ExecModel, Target, assert_conforms, checks, checks_for, checks_for_target, run_suite,
};
use trogon_sandbox_contract::ProviderKind;
use trogon_sandbox_memory_worker::{MemoryWorker, MemoryWorkerConfig};

#[tokio::test(flavor = "multi_thread")]
async fn memory_worker_conforms() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let worker = MemoryWorker::start(client.clone(), MemoryWorkerConfig::default()).await?;
    assert_conforms(&memory_target(client)).await?;
    assert_eq!(drained(&worker).await, 0, "every check releases its lease");
    worker.shutdown().await;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_worker_fails_the_suite_when_it_is_not_running() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let mut target = memory_target(client);
    target.timeout = Duration::from_millis(300);
    let failures = run_suite(&target).await;
    assert_eq!(failures.len(), checks_for(target.exec).len(), "{failures:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_js_eval_target_fails_every_check_it_runs_when_nothing_answers() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let mut target = Target::new(client, ProviderKind::V8Wasm);
    target.exec = ExecModel::JsEval;
    target.timeout = Duration::from_millis(300);
    target.checkpoint_running_unsupported = true;
    target.native_passthrough = true;
    target.redispatch_idempotent = true;
    let failures = run_suite(&target).await;
    assert_eq!(failures.len(), checks_for(target.exec).len(), "{failures:?}");
    Ok(())
}

#[test]
fn checks_are_scoped_to_the_exec_model_a_worker_declares() {
    let names = |exec| -> Vec<&str> { checks_for(exec).into_iter().map(|(name, _)| name).collect() };
    let shell = names(ExecModel::Shell);
    let js = names(ExecModel::JsEval);
    let none = names(ExecModel::None);
    assert!(shell.contains(&"run_returns_stdout_and_exit_zero"));
    assert!(!js.contains(&"run_returns_stdout_and_exit_zero"));
    assert!(js.contains(&"eval_returns_stdout_and_exit_zero"));
    assert!(!shell.contains(&"eval_returns_stdout_and_exit_zero"));
    for lifecycle in ["cancel_releases_the_lease", "redispatching_a_lease_keeps_its_state"] {
        assert!(shell.contains(&lifecycle) && js.contains(&lifecycle), "{lifecycle}");
    }
    assert!(none.contains(&"lease_is_granted"));
    assert!(
        none.iter()
            .all(|name| !name.starts_with("run_") && !name.starts_with("eval_"))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn file_verb_checks_follow_the_profile_flags_not_the_exec_model() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let names =
        |target: &Target| -> Vec<&str> { checks_for_target(target).into_iter().map(|(name, _)| name).collect() };
    let mut target = Target::new(client, ProviderKind::Subprocess);
    target.exec = ExecModel::None;
    assert!(!names(&target).contains(&"file_verbs_round_trip"));
    target.file_ops = true;
    let with_ops = names(&target);
    assert!(with_ops.contains(&"file_verbs_round_trip"));
    assert!(!with_ops.contains(&"file_search_finds_names_and_content"));
    assert!(!with_ops.contains(&"file_archive_round_trips_a_tree"));
    assert!(with_ops.iter().all(|name| !name.starts_with("run_")));
    target.file_search = true;
    target.file_archive = true;
    target.exec = ExecModel::JsEval;
    let all = names(&target);
    assert!(all.contains(&"file_search_finds_names_and_content"));
    assert!(all.contains(&"file_archive_round_trips_a_tree"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn the_denied_connect_check_runs_only_for_a_shell_worker_that_declares_it() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let has = |target: &Target| {
        checks_for_target(target)
            .into_iter()
            .any(|(name, _)| name == "denied_connect_fails")
    };
    let mut target = Target::new(client, ProviderKind::Mock);
    assert!(!has(&target));
    target.denied_connect_fails = true;
    assert!(has(&target));
    target.exec = ExecModel::JsEval;
    assert!(!has(&target));
    assert!(checks().into_iter().any(|(name, _)| name == "denied_connect_fails"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a running local docker worker and SANDBOX_CONFORMANCE_NATS_URL, see the module docs"]
async fn local_docker_worker_conforms() -> Result<(), Box<dyn Error>> {
    let mut target = external_target(ProviderKind::LocalDocker).await?;
    target.supports_pause_resume = true;
    target.graceful_stop = true;
    target.command_restart = true;
    target.named_processes = true;
    target.process_readiness = true;
    target.native_passthrough = true;
    target.redispatch_idempotent = true;
    target.file_ops = true;
    target.file_search = true;
    target.file_archive = true;
    target.denied_connect_fails = true;
    assert_conforms(&target).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a running subprocess worker on Linux or macOS and SANDBOX_CONFORMANCE_NATS_URL, see the module docs"]
async fn subprocess_worker_conforms() -> Result<(), Box<dyn Error>> {
    let mut target = external_target(ProviderKind::Subprocess).await?;
    target.exec = ExecModel::None;
    target.redispatch_idempotent = true;
    target.file_ops = true;
    target.file_search = true;
    target.file_archive = true;
    "/workspace".clone_into(&mut target.file_root);
    assert_conforms(&target).await?;
    Ok(())
}

/// Spike: prints every check's verdict before asserting, so one run shows the whole fit of the hluk worker.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs a running hluk worker on a KVM host and SANDBOX_CONFORMANCE_NATS_URL, see experiments/hluk-worker-spike"]
async fn hluk_worker_conforms() -> Result<(), Box<dyn Error>> {
    let mut target = external_target(ProviderKind::Hluk).await?;
    // The first lease pulls the runtime image and saves its warm snapshot.
    target.timeout = Duration::from_secs(120);
    target.strict_unknown_command_exit = true;
    target.checkpoint_running_unsupported = true;
    target.redispatch_idempotent = true;
    target.denied_connect_fails = true;
    let failures = run_suite(&target).await;
    let mut report = String::new();
    for (name, _) in checks_for_target(&target) {
        match failures.iter().find(|failure| failure.name == name) {
            Some(failure) => {
                let why = failure.failure.to_string().replace(['\n', '|'], " ");
                report.push_str(&format!("| {name} | fail | {why} |\n"));
            }
            None => report.push_str(&format!("| {name} | pass | |\n")),
        }
    }
    std::io::Write::write_all(&mut std::io::stdout(), report.as_bytes())?;
    assert!(failures.is_empty(), "{} checks failed", failures.len());
    Ok(())
}
