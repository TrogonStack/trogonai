//! Cancelling a lease must not stall the leases launched after it.

#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::error::Error;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::net::unix::pipe::{Receiver, Sender};
use trogon_sandbox_contract::{LeaseId, LeaseRequest};
use trogon_sandbox_protocol::{AuthorityHost, FreezeOutcome, Provider, PurgeOutcome};
use trogon_sandbox_subprocess_worker::worker_authority::SubprocessAuthorityHost;
use trogon_sandbox_subprocess_worker::{FuseConfig, IsolationBackendKind, SubprocessWorker, WorkerConfig};

type TestResult = Result<(), Box<dyn Error>>;

const REPLY_LIMIT: usize = 128;

/// A pair of named pipes through which the test talks to one owned child.
struct AuthorityPipe {
    request: Sender,
    reply: Receiver,
    request_path: PathBuf,
    reply_path: PathBuf,
}

fn make_fifo(path: &Path) -> Result<(), Box<dyn Error>> {
    nix::unistd::mkfifo(path, nix::sys::stat::Mode::from_bits_truncate(0o600))?;
    Ok(())
}

fn open_read_write(path: &Path) -> Result<std::fs::File, Box<dyn Error>> {
    Ok(std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?)
}

impl AuthorityPipe {
    fn new(root: &Path, name: &str) -> Result<Self, Box<dyn Error>> {
        let request_path = root.join(format!("{name}-request"));
        let reply_path = root.join(format!("{name}-reply"));
        make_fifo(&request_path)?;
        make_fifo(&reply_path)?;
        Ok(Self {
            request: Sender::from_file(open_read_write(&request_path)?)?,
            reply: Receiver::from_file(open_read_write(&reply_path)?)?,
            request_path,
            reply_path,
        })
    }

    async fn exchange(&self, value: &str) -> Result<String, Box<dyn Error>> {
        let bytes = format!("{value}\n");
        let mut sent = 0;
        while sent < bytes.len() {
            self.request.writable().await?;
            match self.request.try_write(&bytes.as_bytes()[sent..]) {
                Ok(count) => sent += count,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
        }
        let mut reply = Vec::new();
        loop {
            self.reply.readable().await?;
            let mut chunk = [0_u8; REPLY_LIMIT];
            match self.reply.try_read(&mut chunk) {
                Ok(0) => return Err("owned child reply channel ended".into()),
                Ok(count) => {
                    reply.extend_from_slice(&chunk[..count]);
                    if reply.ends_with(b"\n") {
                        return Ok(String::from_utf8(reply)?.trim_end().to_string());
                    }
                    if reply.len() > REPLY_LIMIT {
                        return Err("owned child reply is bounded".into());
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error.into()),
            }
        }
    }
}

fn path_str(path: &Path) -> Result<&str, Box<dyn Error>> {
    Ok(path.to_str().ok_or("path is not UTF-8")?)
}

fn lease_request(agent: &str, env: Value) -> Result<LeaseRequest, serde_json::Error> {
    serde_json::from_value(json!({
        "tenant": "t",
        "provider_pref": ["subprocess"],
        "spec": {
            "image": {
                "kind": "registry",
                "reference": { "registry": "", "repository": agent },
            },
            "workspace": { "empty": {} },
            "cpu_seconds": 2,
            "memory_mib": 512,
            "env": env,
            "mounts": [],
        },
        "ttl_seconds": 60,
        "idempotency_key": "k",
        "persistent": true,
    }))
}

fn worker_in(root: &Path, agent: &Path) -> Result<SubprocessWorker, Box<dyn Error>> {
    Ok(SubprocessWorker::new(WorkerConfig {
        agent_binary_default: agent.to_path_buf(),
        workspace_root: root.join("workspaces"),
        isolation_backend: IsolationBackendKind::None,
        bwrap_binary_path: None,
        egress_proxy_binary_path: None,
        relay_binary_path: None,
        fuse: FuseConfig {
            enabled: false,
            mount_s3_binary: PathBuf::from("mount-s3"),
            region: String::new(),
            endpoint: None,
            allow_other: false,
        },
    })?)
}

fn write_script(path: &Path, body: &str) -> TestResult {
    std::fs::write(path, body)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

fn nonce() -> String {
    uuid::Uuid::now_v7().simple().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn authority_purge_tracks_the_owned_child_and_cannot_cancel_a_reused_lease_id() -> TestResult {
    let root = tempfile::tempdir()?;
    let agent = root.path().join("authority-agent.sh");
    write_script(
        &agent,
        "#!/bin/sh\nwhile IFS= read -r value; do printf '%s\\n' \"$value\"; done < \"$AUTHORITY_REQUEST_FIFO\" > \"$AUTHORITY_REPLY_FIFO\"\n",
    )?;
    let worker = Arc::new(worker_in(root.path(), &agent)?);
    let host = SubprocessAuthorityHost::new(worker.clone());
    let id = LeaseId::new("authority-reused-lease");
    let request = |pipe: &AuthorityPipe| -> Result<LeaseRequest, Box<dyn Error>> {
        Ok(lease_request(
            path_str(&agent)?,
            json!({
                "AUTHORITY_REQUEST_FIFO": path_str(&pipe.request_path)?,
                "AUTHORITY_REPLY_FIFO": path_str(&pipe.reply_path)?,
            }),
        )?)
    };
    let observation: TestResult = async {
        let first_pipe = AuthorityPipe::new(root.path(), "first")?;
        worker.lease(&request(&first_pipe)?, &id).await?;
        let first = host
            .held_leases()
            .await?
            .into_iter()
            .find(|entry| entry.lease_id() == &id)
            .ok_or("first owned assignment is inventoried")?;
        assert_eq!(host.freeze(&first).await?, FreezeOutcome::Unsupported);
        let first_nonce = nonce();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), first_pipe.exchange(&first_nonce)).await??,
            first_nonce
        );
        worker.cancel(&id).await?;
        assert!(
            host.held_leases().await?.iter().all(|entry| entry != &first),
            "cancel waits for the exact owned task's exit and cleanup"
        );

        let second_pipe = AuthorityPipe::new(root.path(), "second")?;
        worker.lease(&request(&second_pipe)?, &id).await?;
        let second = host
            .held_leases()
            .await?
            .into_iter()
            .find(|entry| entry.lease_id() == &id)
            .ok_or("new owned assignment is inventoried")?;
        assert_ne!(first, second, "same lease ID has a new actual child-task identity");
        let late = host.purge(&first).await?;
        assert!(
            matches!(late, PurgeOutcome::Stale | PurgeOutcome::Settled),
            "the old task cannot authorize purging its replacement"
        );
        let second_nonce = nonce();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), second_pipe.exchange(&second_nonce)).await??,
            second_nonce,
            "the actual replacement child still services its own control channel after late cleanup"
        );
        assert_eq!(host.purge(&second).await?, PurgeOutcome::Purged);
        assert!(host.held_leases().await?.iter().all(|entry| entry != &second));
        assert_eq!(
            late,
            PurgeOutcome::Settled,
            "the exact completed task retains positive settlement proof without touching its replacement"
        );
        Ok(())
    }
    .await;
    worker.cancel(&id).await?;
    observation
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cancelled_lease_does_not_block_the_next_launch() -> TestResult {
    let root = tempfile::tempdir()?;
    // The image names the agent binary, which must stay up until it is killed.
    let agent = root.path().join("agent.sh");
    write_script(&agent, "#!/bin/sh\nexec sleep 1000\n")?;
    let worker = worker_in(root.path(), &agent)?;

    let agent_path = path_str(&agent)?;
    let first = LeaseId::new("cancel-first");
    worker.lease(&lease_request(agent_path, json!({}))?, &first).await?;
    // Let the lease's watcher start waiting on the process, as it has by the
    // time a caller cancels a lease that has been up for a moment.
    tokio::time::sleep(Duration::from_millis(300)).await;
    tokio::time::timeout(Duration::from_secs(10), worker.cancel(&first)).await??;

    let second = LeaseId::new("cancel-second");
    tokio::time::timeout(
        Duration::from_secs(10),
        worker.lease(&lease_request(agent_path, json!({}))?, &second),
    )
    .await??;
    worker.cancel(&second).await?;
    Ok(())
}
