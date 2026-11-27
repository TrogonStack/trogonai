#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::collections::BTreeSet;
use std::io::Write as _;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use futures::{FutureExt, StreamExt};
use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use rand_core::{OsRng, RngCore};
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{
    CreateSandboxGroupRequest, CreateSandboxGroupResponse, CreateSandboxUserRequest, CreateSandboxUserResponse,
    FileKind, FileOp, FileOpRequest, FileOpResponse, GroupMembershipAction, LeaseError, LeaseId, LeaseRequest,
    ModifyGroupMembershipRequest, ModifyGroupMembershipResponse, ProviderDispatchPayload, ProviderKind, ProviderLease,
    SandboxGroupName, SandboxUserName, TenantId, WorkerHeartbeat,
};
use trogon_sandbox_protocol::subjects;
use trogon_sandbox_protocol::{AUTH_HEADER, DispatchSigner, OP_LEASE_DISPATCH, VerbScope};

const TENANT: &str = "subprocess-user-rpc";
const REPLY_WINDOW: Duration = Duration::from_millis(700);

struct OwnedChild(Child);

impl OwnedChild {
    async fn stop(&mut self) -> anyhow::Result<()> {
        if self.0.try_wait()?.is_some() {
            return Ok(());
        }
        self.0.kill()?;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if self.0.try_wait()?.is_some() {
                    return anyhow::Ok(());
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .context("owned fixture child did not exit after kill")??;
        Ok(())
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.try_wait();
    }
}

struct Fixture {
    client: async_nats::Client,
    nats_url: String,
    signer: DispatchSigner,
    workers: Vec<OwnedChild>,
    leases: Vec<LeaseId>,
    pids: BTreeSet<i32>,
    logs: Vec<PathBuf>,
    root: tempfile::TempDir,
    agent: PathBuf,
    _server: JetStreamTestServer,
}

/// The tenant a lease belongs to, as the worker's signed verbs scope it.
fn tenant() -> anyhow::Result<TenantId> {
    Ok(TenantId::new(TENANT)?)
}

/// A new lease id no other lease in this run shares.
fn new_lease_id() -> LeaseId {
    LeaseId::new(uuid::Uuid::now_v7().simple().to_string())
}

/// An executable that stays alive until it is killed, standing in for the
/// agent the worker launches for each lease.
fn write_idle_agent(dir: &Path) -> anyhow::Result<PathBuf> {
    let path = dir.join("idle-agent");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&path)?;
    file.write_all(b"#!/bin/sh\nexec sleep 3600\n")?;
    file.sync_all()?;
    Ok(path)
}

impl Fixture {
    async fn new() -> anyhow::Result<Self> {
        let root = tempfile::tempdir()?;
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        let mut seed = [0u8; 32];
        OsRng.try_fill_bytes(&mut seed)?;
        Ok(Self {
            client,
            nats_url: format!("nats://{}", server.address()),
            signer: DispatchSigner::new("user-rpc-fixture", seed),
            workers: Vec::new(),
            leases: Vec::new(),
            pids: BTreeSet::new(),
            logs: Vec::new(),
            agent: write_idle_agent(root.path())?,
            root,
            _server: server,
        })
    }

    fn start_worker(&mut self, agent: &Path) -> anyhow::Result<()> {
        let index = self.workers.len();
        let log_path = self.root.path().join(format!("worker-{index}.log"));
        let log = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&log_path)?;
        let worker = Command::new(env!("CARGO_BIN_EXE_trogon-sandbox-subprocess-worker"))
            .arg("--nats-url")
            .arg(&self.nats_url)
            .arg("--agent-binary")
            .arg(agent)
            .args(["--isolation-backend", "none"])
            .args(["--metrics-addr", "127.0.0.1:0"])
            .arg("--workspace-root")
            .arg(self.root.path().join(format!("workspace-{index}")))
            .env("SANDBOX_DISPATCH_VERIFY", "require")
            .env(
                "SANDBOX_DISPATCH_PUBLIC_KEYS",
                format!("user-rpc-fixture:{}", self.signer.public_key_b64()),
            )
            .env_remove("SANDBOX_DISPATCH_PUBLIC_KEYS_FILE")
            .env(
                "SANDBOX_DISPATCH_FENCE_FILE",
                self.root.path().join(format!("fence-{index}.jsonl")),
            )
            .env("SANDBOX_FENCE_EPOCH", "warn")
            .env("SANDBOX_WORKER_AUTHORITY", "off")
            .env("SANDBOX_FUSE_ENABLED", "false")
            .env("SANDBOX_REQUIRE_COMMITTED_POLICY", "false")
            .env_remove("SANDBOX_OTLP_ENDPOINT")
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .spawn()?;
        self.workers.push(OwnedChild(worker));
        self.logs.push(log_path);
        Ok(())
    }

    fn headers(&self, subject: &str, lease: &LeaseId, body: &[u8]) -> anyhow::Result<async_nats::HeaderMap> {
        let scope = VerbScope {
            tenant: tenant()?,
            lease_id: lease.clone(),
            worker_id: None,
            attempt_id: None,
            epoch: None,
        };
        let mut headers = async_nats::HeaderMap::new();
        headers.insert(AUTH_HEADER, self.signer.sign_verb(subject, body, &scope)?.as_str());
        Ok(headers)
    }

    async fn collect(&self, subject: String, lease: &LeaseId, body: Vec<u8>) -> anyhow::Result<Vec<Vec<u8>>> {
        let inbox = self.client.new_inbox();
        let mut replies = self.client.subscribe(inbox.clone()).await?;
        self.client.flush().await?;
        self.client
            .publish_with_reply_and_headers(
                subject.clone(),
                inbox,
                self.headers(&subject, lease, &body)?,
                body.into(),
            )
            .await?;
        self.client.flush().await?;
        let deadline = tokio::time::Instant::now() + REPLY_WINDOW;
        let mut bodies = Vec::new();
        while let Ok(reply) = tokio::time::timeout_at(deadline, replies.next()).await {
            let reply = reply.context("fixture reply subscription closed")?;
            bodies.push(reply.payload.to_vec());
        }
        Ok(bodies)
    }

    async fn allocate(&mut self) -> anyhow::Result<(LeaseId, ProviderLease)> {
        let lease_id = new_lease_id();
        self.leases.push(lease_id.clone());
        let agent_path = self.agent.to_str().context("agent path is not utf-8")?;
        let request: LeaseRequest = serde_json::from_value(serde_json::json!({
            "tenant": TENANT,
            "provider_pref": ["subprocess"],
            "spec": {
                "image": {
                    "kind": "registry",
                    "reference": { "registry": "", "repository": agent_path, "tag": "latest" },
                },
                "workspace": { "empty": {} },
                "cpu_seconds": 2,
                "memory_mib": 512,
                "env": {},
                "mounts": [],
            },
            "ttl_seconds": 60,
            "idempotency_key": lease_id.as_str(),
        }))?;
        let dispatch = ProviderDispatchPayload {
            lease_id: lease_id.clone(),
            request,
            correlation_id: None,
            resolved_drives: Vec::new(),
            target: None,
        };
        let body = self
            .signer
            .sign_payload(&serde_json::to_vec(&dispatch)?, OP_LEASE_DISPATCH)?;
        let reply = self
            .client
            .send_request(
                subjects::provider_dispatch(ProviderKind::Subprocess),
                async_nats::Request::new()
                    .payload(body.into())
                    .timeout(Some(Duration::from_secs(5))),
            )
            .await?;
        let grant: ProviderLease =
            serde_json::from_slice(&reply.payload).context("actual worker must grant a native idle-agent lease")?;
        let pid: i32 = grant.sandbox_id.as_str().parse().context("host-native PID grant")?;
        ensure!(pid > 1, "native PID must be a specific child process");
        self.pids.insert(pid);
        ensure!(native_present(pid)?, "granted native agent must be alive");
        Ok((lease_id, grant))
    }

    async fn assert_workspace(&self, lease: &LeaseId) -> anyhow::Result<()> {
        let replies = self
            .collect(
                subjects::provider_file_op(ProviderKind::Subprocess, lease),
                lease,
                serde_json::to_vec(&FileOpRequest {
                    lease_id: lease.clone(),
                    op: FileOp::Stat {
                        path: "/workspace".into(),
                    },
                    as_user: None,
                })?,
            )
            .await?;
        ensure!(
            replies.len() == 1,
            "live workspace must have exactly one owner: {replies:?}"
        );
        let response: FileOpResponse = serde_json::from_slice(&replies[0])?;
        ensure!(
            matches!(response, FileOpResponse::Stat { stat } if stat.kind == FileKind::Directory),
            "live WorkspaceRegistry ownership control must stat the workspace directory"
        );
        Ok(())
    }

    async fn cancel(&self, lease: &LeaseId) -> anyhow::Result<()> {
        let subject = subjects::provider_cancel(ProviderKind::Subprocess, lease);
        self.client
            .publish_with_headers(subject.clone(), self.headers(&subject, lease, &[])?, Vec::new().into())
            .await?;
        self.client.flush().await?;
        Ok(())
    }

    async fn finish(&mut self) -> Vec<String> {
        let mut failures = Vec::new();
        for lease in &self.leases {
            if let Err(error) = self.cancel(lease).await {
                failures.push(format!("signed lease cleanup failed: {error:#}"));
            }
        }
        for &pid in &self.pids {
            if let Err(error) = wait_absent(pid).await {
                failures.push(format!("native cleanup unconfirmed for {pid}: {error:#}"));
            }
        }
        for worker in &mut self.workers {
            if let Err(error) = worker.stop().await {
                failures.push(format!("owned worker cleanup failed: {error:#}"));
            }
        }
        failures
    }
}

fn native_present(pid: i32) -> anyhow::Result<bool> {
    // Signal zero observes this fixture's same-host native grants without terminating them.
    match kill(Pid::from_raw(pid), None) {
        Ok(()) => Ok(true),
        Err(Errno::ESRCH) => Ok(false),
        Err(error) => Err(error).context("native presence is unknown, not absent"),
    }
}

async fn wait_absent(pid: i32) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while native_present(pid)? {
        ensure!(Instant::now() < deadline, "native PID remains present");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug)]
enum UserVerb {
    CreateUser,
    CreateGroup,
    AddMembership,
    RemoveMembership,
}

impl UserVerb {
    fn subject(self, lease: &LeaseId) -> String {
        match self {
            Self::CreateUser => subjects::provider_create_user(ProviderKind::Subprocess, lease),
            Self::CreateGroup => subjects::provider_create_group(ProviderKind::Subprocess, lease),
            Self::AddMembership | Self::RemoveMembership => {
                subjects::provider_modify_group_membership(ProviderKind::Subprocess, lease)
            }
        }
    }

    fn body(self, lease: &LeaseId) -> anyhow::Result<Vec<u8>> {
        let username = SandboxUserName::new("ownership-fixture")?;
        let groupname = SandboxGroupName::new("ownership-fixture")?;
        Ok(match self {
            Self::CreateUser => serde_json::to_vec(&CreateSandboxUserRequest {
                lease_id: lease.clone(),
                username,
            })?,
            Self::CreateGroup => serde_json::to_vec(&CreateSandboxGroupRequest {
                lease_id: lease.clone(),
                groupname,
            })?,
            Self::AddMembership | Self::RemoveMembership => serde_json::to_vec(&ModifyGroupMembershipRequest {
                lease_id: lease.clone(),
                username,
                groupname,
                action: match self {
                    Self::AddMembership => GroupMembershipAction::Add,
                    _ => GroupMembershipAction::Remove,
                },
            })?,
        })
    }

    fn unsupported(self, body: &[u8]) -> anyhow::Result<bool> {
        let error = match self {
            Self::CreateUser => match serde_json::from_slice(body)? {
                CreateSandboxUserResponse::Denied(error) => Some(error),
                _ => None,
            },
            Self::CreateGroup => match serde_json::from_slice(body)? {
                CreateSandboxGroupResponse::Denied(error) => Some(error),
                _ => None,
            },
            Self::AddMembership | Self::RemoveMembership => match serde_json::from_slice(body)? {
                ModifyGroupMembershipResponse::Denied(error) => Some(error),
                _ => None,
            },
        };
        Ok(
            matches!(error, Some(LeaseError::Backend { message }) if message.contains("real per-agent Linux users are not supported on this provider")),
        )
    }
}

async fn observe(fixture: &mut Fixture, agent: &Path) -> anyhow::Result<Vec<String>> {
    let mut heartbeats = fixture
        .client
        .subscribe(subjects::worker_heartbeat_filter(ProviderKind::Subprocess))
        .await?;
    fixture.client.flush().await?;
    fixture.start_worker(agent)?;
    fixture.start_worker(agent)?;
    let identities = tokio::time::timeout(Duration::from_secs(8), async {
        let mut identities = BTreeSet::new();
        while identities.len() < 2 {
            let message = heartbeats.next().await.context("heartbeat feed closed")?;
            let heartbeat: WorkerHeartbeat = serde_json::from_slice(&message.payload)?;
            identities.insert(heartbeat.worker_id.as_str().to_string());
        }
        anyhow::Ok(identities)
    })
    .await??;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if fixture
                .logs
                .iter()
                .all(|log| std::fs::read_to_string(log).is_ok_and(|log| log.contains("worker ready")))
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .context("both actual workers must finish subscribing")?;
    let (target, grant) = fixture.allocate().await?;
    let (sentinel, sentinel_grant) = fixture.allocate().await?;
    for grant in [&grant, &sentinel_grant] {
        ensure!(
            grant
                .worker_id
                .as_ref()
                .is_some_and(|worker| identities.contains(worker.as_str())),
            "grant owner must match an observed actual worker heartbeat"
        );
    }
    ensure!(
        grant.sandbox_id != sentinel_grant.sandbox_id,
        "target and sentinel must be distinct native processes"
    );
    fixture.assert_workspace(&target).await?;
    fixture.assert_workspace(&sentinel).await?;
    let unknown = new_lease_id();
    let verbs = [
        UserVerb::CreateUser,
        UserVerb::CreateGroup,
        UserVerb::AddMembership,
        UserVerb::RemoveMembership,
    ];
    let mut failures = Vec::new();
    for verb in verbs {
        let unknown_replies = fixture
            .collect(verb.subject(&unknown), &unknown, verb.body(&unknown)?)
            .await?;
        ensure!(
            unknown_replies.is_empty(),
            "{verb:?} unknown lease must not have an owner: {unknown_replies:?}"
        );
        let mismatch_replies = fixture
            .collect(verb.subject(&unknown), &unknown, verb.body(&target)?)
            .await?;
        ensure!(
            mismatch_replies.is_empty(),
            "{verb:?} must not route a signed unknown subject to a live body lease: {mismatch_replies:?}"
        );
        let live_replies = fixture
            .collect(verb.subject(&target), &target, verb.body(&target)?)
            .await?;
        if live_replies.len() != 1 {
            failures.push(format!(
                "{verb:?}: expected exactly one typed unsupported reply from the live owner, got {}",
                live_replies.len()
            ));
        } else if !verb.unsupported(&live_replies[0])? {
            failures.push(format!(
                "{verb:?}: live owner did not return the existing typed unsupported refusal"
            ));
        }
        for &pid in &fixture.pids {
            ensure!(
                native_present(pid)?,
                "unsupported user RPC must preserve target and unrelated sentinel"
            );
        }
    }
    fixture.assert_workspace(&target).await?;
    fixture.assert_workspace(&sentinel).await?;
    fixture.cancel(&target).await?;
    wait_absent(grant.sandbox_id.as_str().parse()?).await?;
    for verb in verbs {
        let replies = fixture
            .collect(verb.subject(&target), &target, verb.body(&target)?)
            .await?;
        ensure!(
            replies.is_empty(),
            "{verb:?} canceled lease must no longer have a reply owner: {replies:?}"
        );
    }
    ensure!(
        native_present(sentinel_grant.sandbox_id.as_str().parse()?)?,
        "canceling target must preserve unrelated sentinel"
    );
    fixture.assert_workspace(&sentinel).await?;
    Ok(failures)
}

impl Fixture {
    /// What the workers wrote, for the message of a failed run.
    fn worker_logs(&self) -> String {
        self.logs
            .iter()
            .map(|log| std::fs::read_to_string(log).unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[tokio::test]
async fn granted_live_user_rpcs_have_exactly_one_unsupported_owner() -> anyhow::Result<()> {
    let mut fixture = Fixture::new().await?;
    let agent = fixture.agent.clone();
    let observed = std::panic::AssertUnwindSafe(observe(&mut fixture, &agent))
        .catch_unwind()
        .await;
    let cleanup = fixture.finish().await;
    ensure!(cleanup.is_empty(), "strict fixture cleanup failed: {cleanup:?}");
    let failures = match observed {
        Ok(result) => result.with_context(|| {
            format!(
                "native ownership controls failed; worker logs:\n{}",
                fixture.worker_logs()
            )
        })?,
        Err(_) => anyhow::bail!(
            "native ownership case panicked; fixture cleanup completed; worker logs:\n{}",
            fixture.worker_logs()
        ),
    };
    ensure!(
        failures.is_empty(),
        "live WorkspaceRegistry/native/nonowner controls passed and both native leases are absent; semantic user-RPC failures={failures:?}"
    );
    Ok(())
}
