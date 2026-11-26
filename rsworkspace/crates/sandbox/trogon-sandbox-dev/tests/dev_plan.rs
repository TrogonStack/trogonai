use std::error::Error;
use std::path::{Path, PathBuf};

use clap::Parser;
use trogon_sandbox_dev::WorkerKind::{Auto, Docker, Memory, Subprocess};
use trogon_sandbox_dev::{DevArgs, DevPlan, PlanError, WorkerKind, find_binary};

type TestResult = Result<(), Box<dyn Error>>;

fn args(extra: &[&str]) -> Result<DevArgs, clap::Error> {
    let mut argv = vec!["trogon-sandbox-dev"];
    argv.extend_from_slice(extra);
    DevArgs::try_parse_from(argv)
}

fn fake(name: &str) -> Option<PathBuf> {
    Some(PathBuf::from(format!("/bin/{name}")))
}

fn plan_for(args: &DevArgs, worker: WorkerKind) -> Result<DevPlan, PlanError> {
    args.plan(worker, Path::new("/store"), Path::new("/ws"), fake)
}

#[test]
fn auto_resolves_to_docker_then_subprocess_then_memory() {
    assert_eq!(Auto.resolve(true, true, true), Docker);
    assert_eq!(Auto.resolve(false, true, true), Subprocess);
    assert_eq!(Auto.resolve(false, false, true), Memory);
    assert_eq!(Auto.resolve(false, true, false), Memory);
    assert_eq!(Memory.resolve(true, true, true), Memory);
    assert_eq!(Docker.resolve(false, false, false), Docker);
}

#[test]
fn plan_wires_every_process_to_the_same_nats() -> TestResult {
    let args = args(&["--nats-port", "5555", "--gateway-listen", "127.0.0.1:9000"])?;
    let plan = plan_for(&args, WorkerKind::Docker)?;
    assert_eq!(plan.nats_url, "nats://127.0.0.1:5555");
    assert_eq!(plan.gateway_http_url, "http://127.0.0.1:9000");
    assert_eq!(plan.gateway_ws_url, "ws://127.0.0.1:9000");
    assert_eq!(plan.worker, WorkerKind::Docker);
    assert!(plan.nats.args.windows(2).any(|pair| pair == ["-p", "5555"]));
    assert!(plan.nats.args.contains(&"-js".to_string()));
    let [broker, worker, gateway] = &plan.services[..] else {
        return Err("expected three services".into());
    };
    assert_eq!(broker.program, PathBuf::from("/bin/trogon-sandbox-broker"));
    assert_eq!(broker.env["NATS_URL"], plan.nats_url);
    assert_eq!(broker.env["SANDBOX_QUOTA_BACKEND"], "in-memory");
    assert_eq!(broker.env["SANDBOX_JETSTREAM_REPLICAS"], "1");
    assert_eq!(broker.env["SANDBOX_INSECURE_DEV"], "true");
    assert_eq!(worker.program, PathBuf::from("/bin/trogon-sandbox-docker-worker"));
    assert_eq!(worker.env["SANDBOX_NATS_URL"], plan.nats_url);
    assert_eq!(worker.env["SANDBOX_DISPATCH_VERIFY"], "require");
    assert!(!broker.env["SANDBOX_DISPATCH_SIGNING_KEY"].is_empty());
    assert!(worker.env["SANDBOX_DISPATCH_PUBLIC_KEYS"].starts_with("k1:"));
    assert_eq!(worker.env["SANDBOX_IMAGE"], "alpine:3.20");
    assert!(worker.env["SANDBOX_DOCKER_STATE_DIR"].ends_with("docker-worker"));
    assert!(
        gateway
            .args
            .windows(2)
            .any(|pair| pair == ["--nats-url", plan.nats_url.as_str()])
    );
    assert!(
        gateway
            .args
            .windows(2)
            .any(|pair| pair == ["--listen", "127.0.0.1:9000"])
    );
    Ok(())
}

#[test]
fn every_plan_mints_a_fresh_dispatch_key() -> TestResult {
    let args = args(&[])?;
    let first = plan_for(&args, WorkerKind::Memory)?;
    let second = plan_for(&args, WorkerKind::Memory)?;
    assert_ne!(
        first.services[0].env["SANDBOX_DISPATCH_SIGNING_KEY"],
        second.services[0].env["SANDBOX_DISPATCH_SIGNING_KEY"]
    );
    Ok(())
}

#[test]
fn memory_worker_gets_no_backend_settings() -> TestResult {
    let plan = plan_for(&args(&[])?, WorkerKind::Memory)?;
    let worker = &plan.services[1];
    assert_eq!(worker.program, PathBuf::from("/bin/trogon-sandbox-memory-worker"));
    assert!(!worker.env.contains_key("SANDBOX_IMAGE"));
    assert!(!worker.env.contains_key("SANDBOX_AGENT_BINARY"));
    Ok(())
}

#[test]
fn subprocess_worker_requires_agent_binary() -> TestResult {
    let error = plan_for(&args(&[])?, WorkerKind::Subprocess);
    assert_eq!(error, Err(PlanError::AgentBinaryRequired));
    let args = args(&["--agent-binary", "/opt/agent", "--isolation-backend", "none"])?;
    let plan = plan_for(&args, WorkerKind::Subprocess)?;
    let worker = &plan.services[1];
    assert_eq!(worker.program, PathBuf::from("/bin/trogon-sandbox-subprocess-worker"));
    assert_eq!(worker.env["SANDBOX_AGENT_BINARY"], "/opt/agent");
    assert_eq!(worker.env["SANDBOX_WORKSPACE_ROOT"], "/ws");
    assert_eq!(worker.env["SANDBOX_ISOLATION_BACKEND"], "none");
    Ok(())
}

#[test]
fn subprocess_worker_without_a_backend_leaves_it_to_the_worker() -> TestResult {
    let args = args(&["--agent-binary", "/opt/agent"])?;
    let plan = plan_for(&args, WorkerKind::Subprocess)?;
    assert!(!plan.services[1].env.contains_key("SANDBOX_ISOLATION_BACKEND"));
    Ok(())
}

#[test]
fn missing_nats_server_gives_an_install_hint() -> TestResult {
    let error = args(&[])?
        .plan(WorkerKind::Memory, Path::new("/s"), Path::new("/w"), |name| {
            (name != "nats-server").then(|| PathBuf::from(name))
        })
        .err()
        .ok_or("plan must fail")?;
    assert_eq!(error, PlanError::NatsServerNotFound);
    assert!(error.to_string().contains("Install"));
    Ok(())
}

#[test]
fn each_missing_sibling_binary_names_the_build_command() -> TestResult {
    for missing in [
        "trogon-sandbox-broker",
        "trogon-sandbox-memory-worker",
        "trogon-sandbox-ws-gateway",
    ] {
        let error = args(&[])?
            .plan(WorkerKind::Memory, Path::new("/s"), Path::new("/w"), |name| {
                (name != missing).then(|| PathBuf::from(name))
            })
            .err()
            .ok_or("plan must fail")?;
        assert!(matches!(error, PlanError::BinaryNotFound { name } if name == missing));
        assert!(error.to_string().contains("cargo build"));
        assert!(error.to_string().contains(missing));
    }
    Ok(())
}

#[test]
fn flags_parse_into_domain_types() -> TestResult {
    let parsed = args(&["--tenant", "acme", "--api-key", "secret-value"])?;
    assert_eq!(parsed.tenant.as_str(), "acme");
    assert_eq!(parsed.nats_url(), "nats://127.0.0.1:4222");
    let token = parsed.bearer_token()?.ok_or("api key was given")?;
    assert_eq!(token.expose_secret(), "secret-value");
    assert!(args(&[])?.bearer_token()?.is_none());
    Ok(())
}

#[test]
fn an_empty_or_unprintable_api_key_is_not_a_token() -> TestResult {
    assert!(args(&["--api-key", ""])?.bearer_token().is_err());
    assert!(args(&["--api-key", "has space"])?.bearer_token().is_err());
    Ok(())
}

#[test]
fn an_invalid_tenant_is_rejected_when_flags_parse() {
    assert!(args(&["--tenant", ""]).is_err());
}

#[test]
fn debug_output_never_shows_the_api_key() -> TestResult {
    let parsed = args(&["--api-key", "very-secret-key"])?;
    assert!(!format!("{parsed:?}").contains("very-secret-key"));
    Ok(())
}

#[test]
fn find_binary_prefers_bin_dir() -> TestResult {
    let dir = tempfile::tempdir()?;
    let bin = dir.path().join("trogon-sandbox-x");
    std::fs::write(&bin, b"")?;
    assert_eq!(find_binary("trogon-sandbox-x", Some(dir.path())), Some(bin));
    assert_eq!(find_binary("trogon-sandbox-does-not-exist-zz", Some(dir.path())), None);
    assert_eq!(find_binary("trogon-sandbox-does-not-exist-zz", None), None);
    Ok(())
}
