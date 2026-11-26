//! Runs the stack against a NATS server started in a container. `nats-server`, the broker, the worker and the
//! gateway are stand-in scripts, so the tests need no service binaries on PATH: the test points the stack's NATS
//! port at the container, answers the broker's tenant limits request itself and owns the gateway's listening socket.

#![cfg(unix)]

use std::error::Error;
use std::future::Future;
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use clap::Parser;
use futures::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_auth::{ApiKeyStore, BearerToken, NatsKvApiKeyStore};
use trogon_sandbox_contract::{AccountLimits, GetAccountLimitsResponse, LeaseError, TenantId};
use trogon_sandbox_dev::{
    DevArgs, DevError, DevReady, ProcessSpec, RunningProcess, WorkerKind, first_exit, register_api_key, run, spawn,
    wait_for_broker, wait_for_tcp,
};
use trogon_sandbox_protocol::subjects::TENANT_LIMITS_REQUEST;

type TestResult = Result<(), Box<dyn Error>>;

const SLEEPER: &str = "#!/bin/sh\nexec sleep 300\n";

fn free_port() -> Result<u16, std::io::Error> {
    Ok(TcpListener::bind("127.0.0.1:0")?.local_addr()?.port())
}

fn write_script(dir: &Path, name: &str, body: &str) -> Result<(), std::io::Error> {
    let path = dir.join(name);
    std::fs::write(&path, body)?;
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
}

struct Fixture {
    dir: tempfile::TempDir,
    nats_port: u16,
    gateway: TcpListener,
    _nats_listener: Option<TcpListener>,
    _server: Option<JetStreamTestServer>,
}

impl Fixture {
    /// A fixture whose `nats-server` stand-in port is a plain listener; for tests that never reach NATS.
    fn new(broker: &str, worker: &str) -> Result<Self, std::io::Error> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        Self::build(broker, worker, port, Some(listener), None)
    }

    /// A fixture whose stack talks to a real NATS server running in a container.
    async fn with_server(broker: &str, worker: &str) -> Result<Self, Box<dyn Error>> {
        let server = JetStreamTestServer::start().await;
        let port = server
            .address()
            .rsplit(':')
            .next()
            .ok_or("server address has no port")?
            .parse()?;
        Ok(Self::build(broker, worker, port, None, Some(server))?)
    }

    fn build(
        broker: &str,
        worker: &str,
        nats_port: u16,
        listener: Option<TcpListener>,
        server: Option<JetStreamTestServer>,
    ) -> Result<Self, std::io::Error> {
        let dir = tempfile::tempdir()?;
        let nats = format!(
            "#!/bin/sh\necho $$ > '{}'\nexec sleep 300\n",
            dir.path().join("nats.pid").display()
        );
        write_script(dir.path(), "nats-server", &nats)?;
        write_script(dir.path(), "trogon-sandbox-broker", broker)?;
        write_script(dir.path(), "trogon-sandbox-memory-worker", worker)?;
        write_script(dir.path(), "trogon-sandbox-ws-gateway", SLEEPER)?;
        Ok(Self {
            dir,
            nats_port,
            gateway: TcpListener::bind("127.0.0.1:0")?,
            _nats_listener: listener,
            _server: server,
        })
    }

    /// Whether the `nats-server` stand-in is not running: never started, or already stopped.
    async fn nats_stopped(&self) -> Result<bool, Box<dyn Error>> {
        let Ok(pid) = std::fs::read_to_string(self.dir.path().join("nats.pid")) else {
            return Ok(true);
        };
        let alive = Command::new("kill")
            .args(["-0", pid.trim()])
            .output()
            .await?
            .status
            .success();
        Ok(!alive)
    }

    fn nats_url(&self) -> String {
        format!("nats://127.0.0.1:{}", self.nats_port)
    }

    fn flags(&self) -> Result<Vec<String>, std::io::Error> {
        Ok(vec![
            "--worker".into(),
            "memory".into(),
            "--nats-port".into(),
            self.nats_port.to_string(),
            "--gateway-listen".into(),
            self.gateway.local_addr()?.to_string(),
            "--bin-dir".into(),
            self.dir.path().display().to_string(),
        ])
    }

    fn args(&self, extra: &[&str]) -> Result<DevArgs, Box<dyn Error>> {
        let mut argv = vec!["trogon-sandbox-dev".to_owned()];
        argv.extend(self.flags()?);
        argv.extend(extra.iter().map(|flag| (*flag).to_owned()));
        Ok(DevArgs::try_parse_from(argv)?)
    }
}

fn limits_reply() -> Result<Vec<u8>, serde_json::Error> {
    serde_json::to_vec(&GetAccountLimitsResponse::Limits {
        limits: AccountLimits {
            active_leases: 0,
            max_active_leases: 4,
            cpu_seconds_used: 0,
            cpu_seconds_cap: 100,
            wall_seconds_used: 0,
            wall_seconds_cap: 100,
            max_cpu_seconds_per_request: 10,
            max_memory_mib_per_request: 512,
            per_tag: None,
        },
    })
}

/// Answers every tenant limits request with `reply`, retrying the connection until the server is up.
fn answer_limits(nats_url: String, reply: Vec<u8>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let client = loop {
            if let Ok(client) = async_nats::connect(&nats_url).await {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        let Ok(mut requests) = client.subscribe(TENANT_LIMITS_REQUEST).await else {
            return;
        };
        while let Some(request) = requests.next().await {
            if let Some(subject) = request.reply {
                let _ = client.publish(subject, reply.clone().into()).await;
            }
        }
    })
}

async fn within<T>(seconds: u64, future: impl Future<Output = T>) -> Result<T, Box<dyn Error>> {
    Ok(tokio::time::timeout(Duration::from_secs(seconds), future).await?)
}

struct StackRun {
    ready: oneshot::Receiver<DevReady>,
    stop: oneshot::Sender<()>,
    done: JoinHandle<Result<(), DevError>>,
}

fn start_stack(args: DevArgs) -> StackRun {
    let (ready_tx, ready) = oneshot::channel();
    let (stop, stop_rx) = oneshot::channel::<()>();
    let done = tokio::spawn(run(
        args,
        async move {
            let _ = stop_rx.await;
        },
        move |info| {
            let _ = ready_tx.send(info.clone());
        },
    ));
    StackRun { ready, stop, done }
}

#[tokio::test]
async fn the_stack_registers_its_key_announces_readiness_and_stops_cleanly() -> TestResult {
    let fixture = Fixture::with_server(SLEEPER, SLEEPER).await?;
    let responder = answer_limits(fixture.nats_url(), limits_reply()?);
    let key_file = fixture.dir.path().join("key-out");
    let key_file_flag = key_file.display().to_string();
    let stack = start_stack(fixture.args(&[
        "--tenant",
        "acme",
        "--api-key",
        "supplied-key",
        "--api-key-file",
        &key_file_flag,
    ])?);

    let ready = within(75, stack.ready).await??;
    assert_eq!(ready.worker, WorkerKind::Memory);
    assert_eq!(ready.tenant.as_str(), "acme");
    assert_eq!(ready.api_key_file, key_file);
    let banner = ready.to_string();
    assert!(banner.contains("process.env.SANDBOX_API_KEY"));
    assert!(banner.contains(&fixture.nats_url()));
    assert!(!banner.contains("supplied-key"));
    assert_eq!(std::fs::read_to_string(&key_file)?, "supplied-key");
    assert_eq!(std::fs::metadata(&key_file)?.permissions().mode() & 0o777, 0o600);

    let client = async_nats::connect(fixture.nats_url()).await?;
    let store = NatsKvApiKeyStore::new(async_nats::jetstream::new(client)).await?;
    let record = store
        .lookup(&BearerToken::new("supplied-key")?.hash())
        .await?
        .ok_or("the supplied key is registered")?;
    assert_eq!(record.tenant().as_str(), "acme");
    assert_eq!(record.service_account().as_str(), "dev");

    stack.stop.send(()).map_err(|()| "run already ended")?;
    within(20, stack.done).await???;
    assert!(fixture.nats_stopped().await?, "nats-server is stopped with the stack");
    responder.abort();
    Ok(())
}

#[tokio::test]
async fn a_generated_key_is_registered_and_kept_out_of_the_banner() -> TestResult {
    let fixture = Fixture::with_server(SLEEPER, SLEEPER).await?;
    let responder = answer_limits(fixture.nats_url(), limits_reply()?);
    let stack = start_stack(fixture.args(&[])?);

    let ready = within(75, stack.ready).await??;
    let key = std::fs::read_to_string(&ready.api_key_file)?;
    assert!(key.starts_with("sbx-dev-"));
    assert!(!ready.to_string().contains(&key));
    let client = async_nats::connect(fixture.nats_url()).await?;
    let store = NatsKvApiKeyStore::new(async_nats::jetstream::new(client)).await?;
    assert!(store.lookup(&BearerToken::new(key)?.hash()).await?.is_some());

    stack.stop.send(()).map_err(|()| "run already ended")?;
    within(20, stack.done).await???;
    responder.abort();
    Ok(())
}

#[tokio::test]
async fn the_stack_stops_when_a_process_exits_after_readiness() -> TestResult {
    let gate = tempfile::tempdir()?;
    let trigger = gate.path().join("exit-now");
    let worker = format!(
        "#!/bin/sh\nwhile [ ! -e '{}' ]; do sleep 0.05; done\n",
        trigger.display()
    );
    let fixture = Fixture::with_server(SLEEPER, &worker).await?;
    let responder = answer_limits(fixture.nats_url(), limits_reply()?);
    let stack = start_stack(fixture.args(&[])?);

    within(75, stack.ready).await??;
    std::fs::write(&trigger, b"")?;
    within(20, stack.done).await???;
    assert!(fixture.nats_stopped().await?);
    responder.abort();
    Ok(())
}

#[tokio::test]
async fn a_process_exiting_before_readiness_fails_startup() -> TestResult {
    let fixture = Fixture::with_server("#!/bin/sh\nexit 3\n", SLEEPER).await?;
    let stack = start_stack(fixture.args(&[])?);
    let result = within(75, stack.done).await??;
    assert!(matches!(
        result,
        Err(DevError::ExitedDuringStartup {
            name: "trogon-sandbox-broker",
            ..
        })
    ));
    assert!(fixture.nats_stopped().await?);
    Ok(())
}

#[tokio::test]
async fn shutdown_during_startup_is_an_orderly_stop() -> TestResult {
    let fixture = Fixture::with_server(SLEEPER, SLEEPER).await?;
    let mut announced = false;
    let result = run(fixture.args(&[])?, std::future::ready(()), |_| announced = true).await;
    assert!(result.is_ok());
    assert!(!announced, "readiness is never announced");
    assert!(fixture.nats_stopped().await?);
    Ok(())
}

#[tokio::test]
async fn missing_binaries_fail_before_anything_starts() -> TestResult {
    let fixture = Fixture::new(SLEEPER, SLEEPER)?;
    std::fs::remove_file(fixture.dir.path().join("trogon-sandbox-ws-gateway"))?;
    let result = run(fixture.args(&[])?, std::future::pending(), |_| {}).await;
    assert!(matches!(
        result,
        Err(DevError::Plan(trogon_sandbox_dev::PlanError::BinaryNotFound {
            name: "trogon-sandbox-ws-gateway"
        }))
    ));
    assert!(fixture.nats_stopped().await?);
    Ok(())
}

#[tokio::test]
async fn an_invalid_api_key_is_refused_before_anything_starts() -> TestResult {
    let fixture = Fixture::new(SLEEPER, SLEEPER)?;
    let result = run(
        fixture.args(&["--api-key", "has space"])?,
        std::future::pending(),
        |_| {},
    )
    .await;
    assert!(matches!(result, Err(DevError::InvalidApiKey(_))));
    assert!(fixture.nats_stopped().await?);
    Ok(())
}

#[tokio::test]
async fn an_occupied_key_file_is_refused_before_anything_starts() -> TestResult {
    let fixture = Fixture::new(SLEEPER, SLEEPER)?;
    let existing = fixture.dir.path().join("existing-key");
    std::fs::write(&existing, b"keep me")?;
    let flag = existing.display().to_string();
    let result = run(
        fixture.args(&["--api-key-file", &flag])?,
        std::future::pending(),
        |_| {},
    )
    .await;
    assert!(matches!(result, Err(DevError::ApiKeyFile { .. })));
    assert_eq!(std::fs::read(&existing)?, b"keep me");
    assert!(fixture.nats_stopped().await?);
    Ok(())
}

#[tokio::test]
async fn waiting_for_a_port_that_never_opens_names_the_service() -> TestResult {
    let port = free_port()?;
    let error = wait_for_tcp(
        "example",
        &format!("127.0.0.1:{port}"),
        Duration::from_millis(250),
        "; hint",
    )
    .await
    .err()
    .ok_or("nothing listens")?;
    assert!(matches!(error, DevError::NotListening { service: "example", .. }));
    assert!(error.to_string().contains("example"));
    assert!(error.to_string().ends_with("; hint"));
    Ok(())
}

#[tokio::test]
async fn waiting_for_an_open_port_succeeds() -> TestResult {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    wait_for_tcp(
        "example",
        &listener.local_addr()?.to_string(),
        Duration::from_secs(1),
        "",
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn spawning_a_missing_program_names_it() -> TestResult {
    let spec = ProcessSpec {
        name: "ghost",
        program: PathBuf::from("/definitely/not/here"),
        args: vec![],
        env: Default::default(),
    };
    let error = spawn(&spec).err().ok_or("spawn must fail")?;
    assert!(matches!(error, DevError::Spawn { name: "ghost", .. }));
    Ok(())
}

#[tokio::test]
async fn first_exit_reports_the_process_that_ended() -> TestResult {
    let sleeper = ProcessSpec {
        name: "sleeper",
        program: PathBuf::from("sleep"),
        args: vec!["30".into()],
        env: Default::default(),
    };
    let quick = ProcessSpec {
        name: "quick",
        program: PathBuf::from("true"),
        args: vec![],
        env: Default::default(),
    };
    let mut running: Vec<RunningProcess> = vec![spawn(&sleeper)?, spawn(&quick)?];
    let (name, status) = within(10, first_exit(&mut running)).await?;
    assert_eq!(name, "quick");
    assert!(status.contains("Ok"));
    Ok(())
}

#[tokio::test]
async fn first_exit_without_processes_never_resolves() {
    let mut none: Vec<RunningProcess> = vec![];
    assert!(
        tokio::time::timeout(Duration::from_millis(100), first_exit(&mut none))
            .await
            .is_err()
    );
}

struct Nats {
    _server: JetStreamTestServer,
    url: String,
}

async fn start_nats() -> Result<Nats, Box<dyn Error>> {
    let server = JetStreamTestServer::start().await;
    let url = format!("nats://{}", server.address());
    Ok(Nats { _server: server, url })
}

#[tokio::test]
async fn broker_readiness_accepts_a_limits_answer() -> TestResult {
    let nats = start_nats().await?;
    let responder = answer_limits(nats.url.clone(), limits_reply()?);
    wait_for_broker(&nats.url, &TenantId::new("acme")?, Duration::from_secs(10)).await?;
    responder.abort();
    Ok(())
}

#[tokio::test]
async fn broker_readiness_reports_a_refusal() -> TestResult {
    let nats = start_nats().await?;
    let denied = serde_json::to_vec(&GetAccountLimitsResponse::Denied(LeaseError::NoProviderAvailable))?;
    let responder = answer_limits(nats.url.clone(), denied);
    let error = wait_for_broker(&nats.url, &TenantId::new("acme")?, Duration::from_secs(10)).await;
    assert!(matches!(error, Err(DevError::BrokerRefused)));
    responder.abort();
    Ok(())
}

#[tokio::test]
async fn broker_readiness_reports_an_unreadable_answer() -> TestResult {
    let nats = start_nats().await?;
    let responder = answer_limits(nats.url.clone(), b"not json".to_vec());
    let error = wait_for_broker(&nats.url, &TenantId::new("acme")?, Duration::from_secs(10)).await;
    assert!(matches!(error, Err(DevError::InvalidBrokerResponse(_))));
    responder.abort();
    Ok(())
}

/// Subscribes to the limits subject and never answers.
fn stay_silent(nats_url: String) -> JoinHandle<()> {
    tokio::spawn(async move {
        let client = loop {
            if let Ok(client) = async_nats::connect(&nats_url).await {
                break client;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        let Ok(mut requests) = client.subscribe(TENANT_LIMITS_REQUEST).await else {
            return;
        };
        while requests.next().await.is_some() {}
    })
}

// The client gives a request ten seconds before it reports a timeout; the readiness deadline is longer.
#[tokio::test]
async fn broker_readiness_reports_a_request_that_the_client_gave_up_on() -> TestResult {
    let nats = start_nats().await?;
    let silent = stay_silent(nats.url.clone());
    let error = within(
        40,
        wait_for_broker(&nats.url, &TenantId::new("acme")?, Duration::from_secs(30)),
    )
    .await?;
    assert!(matches!(error, Err(DevError::BrokerRequest(_))));
    silent.abort();
    Ok(())
}

#[tokio::test]
async fn broker_readiness_gives_up_at_the_deadline() -> TestResult {
    let nats = start_nats().await?;
    let error = wait_for_broker(&nats.url, &TenantId::new("acme")?, Duration::from_millis(400)).await;
    assert!(matches!(error, Err(DevError::BrokerDeadline)));
    Ok(())
}

#[tokio::test]
async fn broker_readiness_reports_an_unreachable_server() -> TestResult {
    let port = free_port()?;
    let error = wait_for_broker(
        &format!("nats://127.0.0.1:{port}"),
        &TenantId::new("acme")?,
        Duration::from_secs(5),
    )
    .await;
    assert!(matches!(error, Err(DevError::NatsConnect(_))));
    Ok(())
}

#[tokio::test]
async fn registering_a_key_needs_a_reachable_server() -> TestResult {
    let port = free_port()?;
    let error = register_api_key(
        &format!("nats://127.0.0.1:{port}"),
        &TenantId::new("acme")?,
        &BearerToken::new("token")?,
    )
    .await;
    assert!(matches!(error, Err(DevError::NatsConnect(_))));
    Ok(())
}

async fn read_until(
    reader: &mut BufReader<tokio::process::ChildStdout>,
    needle: &str,
) -> Result<String, Box<dyn Error>> {
    let mut seen = String::new();
    let mut line = String::new();
    while !seen.contains(needle) {
        line.clear();
        if reader.read_line(&mut line).await? == 0 {
            return Err("output ended before the stack was ready".into());
        }
        seen.push_str(&line);
    }
    Ok(seen)
}

#[tokio::test]
async fn the_binary_serves_until_terminated() -> TestResult {
    let fixture = Fixture::with_server(SLEEPER, SLEEPER).await?;
    let responder = answer_limits(fixture.nats_url(), limits_reply()?);
    let key = "binary-supplied-key";
    let mut child = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-dev"))
        .args(fixture.flags()?)
        .env("SANDBOX_DEV_API_KEY", key)
        .env_remove("SANDBOX_DEV_API_KEY_FILE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdout = BufReader::new(child.stdout.take().ok_or("stdout")?);
    let banner = within(75, read_until(&mut stdout, "Press Ctrl-C to stop everything.")).await??;
    assert!(banner.contains("process.env.SANDBOX_API_KEY"));
    assert!(!banner.contains(key));

    let pid = child.id().ok_or("pid")?;
    let status = Command::new("kill").args(["-TERM", &pid.to_string()]).status().await?;
    assert!(status.success());
    let exit = within(20, child.wait()).await??;
    assert!(exit.success());
    let mut stderr = String::new();
    child.stderr.take().ok_or("stderr")?.read_to_string(&mut stderr).await?;
    assert!(!stderr.contains(key));
    assert!(fixture.nats_stopped().await?);
    responder.abort();
    Ok(())
}

#[tokio::test]
async fn the_binary_reports_startup_errors() -> TestResult {
    let fixture = Fixture::new(SLEEPER, SLEEPER)?;
    std::fs::remove_file(fixture.dir.path().join("trogon-sandbox-broker"))?;
    let output = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-dev"))
        .args(fixture.flags()?)
        .output()
        .await?;
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("trogon-sandbox-broker"));
    Ok(())
}

#[tokio::test]
async fn help_does_not_echo_the_api_key_from_the_environment() -> TestResult {
    let key = "help-must-not-show-this";
    let output = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-dev"))
        .arg("--help")
        .env("SANDBOX_DEV_API_KEY", key)
        .output()
        .await?;
    assert!(output.status.success());
    for bytes in [&output.stdout, &output.stderr] {
        assert!(!String::from_utf8_lossy(bytes).contains(key));
    }
    Ok(())
}

// The death signal follows the thread that forked the child. A caller thread that spawns a child and exits must not
// take the child with it, because a runtime can retire the thread a supervisor task runs on.
#[cfg(target_os = "linux")]
#[test]
fn a_child_outlives_the_thread_that_asked_for_it() -> TestResult {
    let sleeper = ProcessSpec {
        name: "sleeper",
        program: PathBuf::from("sleep"),
        args: vec!["30".into()],
        env: Default::default(),
    };
    let pid = std::thread::spawn(move || -> Result<libc::pid_t, String> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        runtime.block_on(async {
            let running = spawn(&sleeper).map_err(|error| error.to_string())?;
            let pid = running.child.id().ok_or("child already exited")? as libc::pid_t;
            // Keep kill_on_drop from ending the child when the thread exits.
            std::mem::forget(running);
            Ok(pid)
        })
    })
    .join()
    .map_err(|_| "spawning thread panicked")??;

    std::thread::sleep(Duration::from_millis(500));
    let mut status = 0;
    // SAFETY: waitpid on our own child with a valid status pointer.
    let reaped = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
    // SAFETY: signalling and reaping our own child.
    unsafe {
        libc::kill(pid, libc::SIGKILL);
        libc::waitpid(pid, &mut status, 0);
    }
    assert_eq!(reaped, 0, "child died with the thread that asked for it");
    Ok(())
}
