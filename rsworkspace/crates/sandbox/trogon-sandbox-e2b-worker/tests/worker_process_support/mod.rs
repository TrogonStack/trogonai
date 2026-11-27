// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{ProviderDispatchPayload, ProviderKind};
use trogon_sandbox_protocol::{
    AUTH_HEADER, DispatchSigner, EpochMode, OP_LEASE_DISPATCH, VerbScope, nats_transfer, subjects,
};
use trogon_std::env::{ReadEnv, SystemEnv};
use url::Url;

pub type Boxed = Box<dyn std::error::Error + Send + Sync>;

/// How long an interrupted worker may take to exit before the test fails.
const GRACEFUL_EXIT: Duration = Duration::from_secs(30);

/// A worker process of the real binary; it is killed when dropped.
pub struct Process {
    child: Option<Child>,
    pid: u32,
}

impl Process {
    pub fn id(&self) -> u32 {
        self.pid
    }

    pub fn is_alive(&mut self) -> Result<bool, Boxed> {
        match &mut self.child {
            Some(child) => Ok(child.try_wait()?.is_none()),
            None => Ok(false),
        }
    }

    /// Interrupts the process so it shuts down cleanly and writes its coverage data, then waits for it. A process
    /// that does not exit in time is killed and the call fails.
    pub fn stop(&mut self) -> Result<(), Boxed> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        if child.try_wait()?.is_some() {
            return Ok(());
        }
        send_signal("INT", self.pid)?;
        let (done, exited) = mpsc::sync_channel(1);
        std::thread::spawn(move || {
            let _ = done.send(child.wait().map(|_| ()));
        });
        match exited.recv_timeout(GRACEFUL_EXIT) {
            Ok(result) => Ok(result?),
            Err(_) => {
                send_signal("KILL", self.pid)?;
                Err("the worker did not exit after an interrupt".into())
            }
        }
    }

    /// Kills the process abruptly, as a crash would, and waits for it.
    pub fn kill(&mut self) -> Result<(), Boxed> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        if child.try_wait()?.is_none() {
            child.kill()?;
        }
        child.wait()?;
        Ok(())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        let _ = self.kill();
    }
}

fn send_signal(name: &str, pid: u32) -> Result<(), Boxed> {
    let status = Command::new("kill")
        .arg(format!("-{name}"))
        .arg(pid.to_string())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("kill -{name} {pid} failed: {status}").into())
    }
}

/// A throwaway JetStream server and a client connected to it.
pub struct Nats {
    pub server: JetStreamTestServer,
    pub url: String,
    pub client: async_nats::Client,
}

impl Nats {
    pub async fn start() -> Self {
        let server = JetStreamTestServer::start().await;
        let url = format!("nats://{}", server.address());
        let client = server.client().await;
        Self { server, url, client }
    }
}

pub fn worker(
    nats_url: &str,
    api_url: &Url,
    envd_url: &Url,
    ledger: &Path,
    signer: &DispatchSigner,
) -> Result<Process, Boxed> {
    worker_with_http_timeout(nats_url, api_url, envd_url, ledger, signer, 2)
}

pub fn worker_with_http_timeout(
    nats_url: &str,
    api_url: &Url,
    envd_url: &Url,
    ledger: &Path,
    signer: &DispatchSigner,
    timeout_seconds: u64,
) -> Result<Process, Boxed> {
    worker_with_epoch_mode(
        nats_url,
        api_url,
        envd_url,
        ledger,
        signer,
        timeout_seconds,
        EpochMode::Warn,
    )
}

pub fn worker_with_epoch_mode(
    nats_url: &str,
    api_url: &Url,
    envd_url: &Url,
    ledger: &Path,
    signer: &DispatchSigner,
    timeout_seconds: u64,
    epoch_mode: EpochMode,
) -> Result<Process, Boxed> {
    let epoch = match epoch_mode {
        EpochMode::Off => "off",
        EpochMode::Warn => "warn",
        EpochMode::Require => "require",
    };
    let mut command = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-e2b-worker"));
    command
        .env_clear()
        .env("E2B_API_KEY", "fixture-key")
        .env("E2B_API_URL", api_url.as_str())
        .env("E2B_DOMAIN", "e2b.test")
        .env("SANDBOX_NATS_URL", nats_url)
        .env("SANDBOX_E2B_ENVD_URL", envd_url.as_str())
        .env("SANDBOX_E2B_DISPATCH_LEDGER_FILE", ledger)
        .env("SANDBOX_E2B_HTTP_TIMEOUT_SECS", timeout_seconds.to_string())
        .env("SANDBOX_METRICS_ADDR", "127.0.0.1:0")
        .env("SANDBOX_WORKER_AUTHORITY", "off")
        .env("SANDBOX_DISPATCH_VERIFY", "require")
        .env("SANDBOX_FENCE_EPOCH", epoch)
        .env("SANDBOX_DISPATCH_FENCE_FILE", ledger.with_extension("fence"))
        .env(
            "SANDBOX_DISPATCH_PUBLIC_KEYS",
            format!("{}:{}", signer.key_id(), signer.public_key_b64()),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Coverage runs name where each process writes its profile; without it the worker's execution goes unrecorded.
    if let Some(profile) = SystemEnv.var_os("LLVM_PROFILE_FILE") {
        command.env("LLVM_PROFILE_FILE", profile);
    }
    let child = command.spawn()?;
    let pid = child.id();
    Ok(Process {
        child: Some(child),
        pid,
    })
}

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Sends `request` to `subject` signed for `scope` and reads the reply.
pub async fn signed<T: Serialize, R: DeserializeOwned>(
    nats: &Nats,
    signer: &DispatchSigner,
    subject: String,
    request: &T,
    scope: &VerbScope,
) -> Result<R, Boxed> {
    let payload = serde_json::to_vec_pretty(request)?;
    let mut headers = async_nats::HeaderMap::new();
    headers.insert(AUTH_HEADER, signer.sign_verb(&subject, &payload, scope)?);
    let reply = nats_transfer::request(&nats.client, &subject, payload.into(), Some(headers), REQUEST_TIMEOUT).await?;
    Ok(serde_json::from_slice(&reply)?)
}

/// Sends a signed dispatch to the worker its target names and reads the reply.
pub async fn dispatch<R: DeserializeOwned>(
    nats: &Nats,
    signer: &DispatchSigner,
    dispatch: &ProviderDispatchPayload,
    target_worker: &trogon_sandbox_contract::WorkerId,
) -> Result<R, Boxed> {
    let body = signer.sign_payload(&serde_json::to_vec(dispatch)?, OP_LEASE_DISPATCH)?;
    let subject = subjects::worker_dispatch(ProviderKind::E2b, target_worker);
    let reply = nats_transfer::request(&nats.client, &subject, body.into(), None, REQUEST_TIMEOUT).await?;
    Ok(serde_json::from_slice(&reply)?)
}
