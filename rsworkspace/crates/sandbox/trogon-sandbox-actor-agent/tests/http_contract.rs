#![cfg(feature = "server")]

use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use reqwest::{Client, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use trogon_sandbox_actor_agent::{
    AGENT_MIN_PROTOCOL_VERSION_HEADER, AGENT_PROTOCOL_VERSION_HEADER, ActorIdentity, ActorIdentitySource, AgentState,
    BIND_PATH, BindLeaseResponse, EXEC_PATH, EntropyReseeder, ExecFrame, FILES_READ_PATH, FILES_WRITE_PATH,
    GROUP_MEMBERSHIP_PATH, GROUPS_CREATE_PATH, GUEST_PROTOCOL_VERSION, GuestCallFailure, GuestEntropyReseedMethod,
    GuestEntropySeed, GuestProtocolError, IDENTITY_PATH, MIN_SUPPORTED_WORKER_PROTOCOL_VERSION, READYZ_PATH,
    USERS_CREATE_PATH, WORKER_PROTOCOL_VERSION_HEADER, router, serve,
};
use trogon_sandbox_contract::{
    CreateSandboxGroupResponse, CreateSandboxUserResponse, LeaseError, ModifyGroupMembershipResponse, ReadFileResponse,
    RunCommandResponse, WriteFilesResponse,
};

type TestResult = Result<(), Box<dyn Error>>;

struct Agent {
    base: String,
    client: Client,
    cancel: CancellationToken,
}

impl Drop for Agent {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Agent {
    async fn start(state: AgentState) -> Result<Self, Box<dyn Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let base = format!("http://{}", listener.local_addr()?);
        let cancel = CancellationToken::new();
        tokio::spawn(serve(listener, router(Arc::new(state)), cancel.clone()));
        Ok(Self {
            base,
            client: Client::new(),
            cancel,
        })
    }

    async fn with_identity(path: PathBuf) -> Result<Self, Box<dyn Error>> {
        Self::start(AgentState::new(ActorIdentitySource::new(path))).await
    }

    async fn get(&self, path: &str) -> Result<Response, Box<dyn Error>> {
        let request = self.client.get(format!("{}{path}", self.base));
        Ok(request
            .header(WORKER_PROTOCOL_VERSION_HEADER, GUEST_PROTOCOL_VERSION)
            .send()
            .await?)
    }

    async fn post<T: Serialize>(&self, path: &str, body: &T) -> Result<Response, Box<dyn Error>> {
        let request = self.client.post(format!("{}{path}", self.base));
        let request = request.header(WORKER_PROTOCOL_VERSION_HEADER, GUEST_PROTOCOL_VERSION);
        Ok(request.json(body).send().await?)
    }

    async fn answer<T: DeserializeOwned>(&self, path: &str, body: &Value) -> Result<T, Box<dyn Error>> {
        Ok(self.post(path, body).await?.json().await?)
    }

    async fn bind(&self, lease_id: &str, working_dir: Option<String>) -> TestResult {
        let body = json!({"lease_id": lease_id, "env": {"BOUND": "yes"}, "working_dir": working_dir});
        assert_eq!(self.post(BIND_PATH, &body).await?.status(), 204);
        Ok(())
    }

    async fn exec_frames(&self, lease_id: &str, command: &[&str]) -> Result<Vec<ExecFrame>, Box<dyn Error>> {
        let body = self
            .post(EXEC_PATH, &json!({"lease_id": lease_id, "command": command}))
            .await?
            .text()
            .await?;
        body.lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).map_err(Into::into))
            .collect()
    }
}

struct RejectReseed(AtomicU32);

impl EntropyReseeder for RejectReseed {
    fn reseed(&self, _seed: &GuestEntropySeed) -> std::io::Result<()> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "injected reseed failure",
        ))
    }
}

#[derive(Default)]
struct CaptureReseed(Mutex<Vec<[u8; 32]>>);

impl EntropyReseeder for CaptureReseed {
    fn reseed(&self, seed: &GuestEntropySeed) -> std::io::Result<()> {
        self.0
            .lock()
            .map_err(|error| std::io::Error::other(error.to_string()))?
            .push(*seed.as_bytes());
        Ok(())
    }
}

fn is_denied(frames: &[ExecFrame]) -> bool {
    matches!(frames.last(), Some(ExecFrame::Response(RunCommandResponse::Denied(_))))
}

#[tokio::test]
async fn bind_does_not_install_a_lease_when_kernel_reseed_fails() -> TestResult {
    let dir = tempfile::tempdir()?;
    let reseeder = Arc::new(RejectReseed(AtomicU32::new(0)));
    let state =
        AgentState::new(ActorIdentitySource::new(dir.path().join("actor-id"))).with_entropy_reseeder(reseeder.clone());
    let agent = Agent::start(state).await?;
    let body = json!({"lease_id": "must-stay-unbound", "entropy_seed": vec![7; 32]});
    let response = agent.post(BIND_PATH, &body).await?;
    assert_eq!(response.status(), 503);
    assert!(response.text().await?.contains("kernel entropy reseed failed"));
    assert_eq!(reseeder.0.load(Ordering::SeqCst), 1);
    assert!(is_denied(
        &agent.exec_frames("must-stay-unbound", &["/bin/true"]).await?
    ));
    Ok(())
}

#[tokio::test]
async fn seeded_bind_receipts_identify_each_fresh_seed_after_reseed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let reseeder = Arc::new(CaptureReseed::default());
    let state =
        AgentState::new(ActorIdentitySource::new(dir.path().join("actor-id"))).with_entropy_reseeder(reseeder.clone());
    let agent = Agent::start(state).await?;
    let cases = [
        (
            "restore-a",
            7u8,
            "sha256:4bb06f8e4e3a7715d201d573d0aa423762e55dabd61a2c02278fa56cc6d294e0",
        ),
        (
            "restore-b",
            9u8,
            "sha256:8c0cc17a04942cc4f8e0fe0b302606d3108860c126428ba2ceeb5f9ed41c2b05",
        ),
    ];
    for (lease, byte, digest) in cases {
        let body = json!({"lease_id": lease, "entropy_seed": vec![byte; 32]});
        let response = agent.post(BIND_PATH, &body).await?;
        assert_eq!(response.status(), 200);
        let receipt: BindLeaseResponse = response.json().await?;
        assert_eq!(receipt.lease_id.as_str(), lease);
        assert_eq!(receipt.entropy_reseed.seed_digest.as_str(), digest);
        assert_eq!(receipt.entropy_reseed.method, GuestEntropyReseedMethod::LinuxKernelV1);
    }
    assert_eq!(
        *reseeder.0.lock().map_err(|error| error.to_string())?,
        vec![[7; 32], [9; 32]]
    );
    Ok(())
}

#[tokio::test]
async fn bind_rejects_a_malformed_entropy_seed_before_installing_the_lease() -> TestResult {
    let dir = tempfile::tempdir()?;
    let agent = Agent::with_identity(dir.path().join("actor-id")).await?;
    let response = agent
        .post(
            BIND_PATH,
            &json!({"lease_id": "must-stay-unbound", "entropy_seed": [7]}),
        )
        .await?;
    assert_eq!(response.status(), 422);
    assert!(is_denied(
        &agent.exec_frames("must-stay-unbound", &["/bin/true"]).await?
    ));
    Ok(())
}

#[tokio::test]
async fn readyz_is_available_before_any_lease_is_bound() -> TestResult {
    let dir = tempfile::tempdir()?;
    let agent = Agent::with_identity(dir.path().join("actor-id")).await?;
    assert_eq!(agent.get(READYZ_PATH).await?.status(), 200);
    agent.bind("01J0", None).await?;
    assert_eq!(agent.get(READYZ_PATH).await?.status(), 200);
    Ok(())
}

#[tokio::test]
async fn exec_runs_with_the_bound_environment() -> TestResult {
    let dir = tempfile::tempdir()?;
    let agent = Agent::with_identity(dir.path().join("actor-id")).await?;
    agent.bind("01J0", None).await?;
    let frames = agent
        .exec_frames("01J0", &["/bin/sh", "-c", "printf %s $BOUND"])
        .await?;
    match frames.last() {
        Some(ExecFrame::Response(RunCommandResponse::Completed(output))) => {
            assert_eq!(output.exit_code, 0);
            assert_eq!(output.stdout, b"yes");
        }
        other => return Err(format!("expected a completed response, got {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn exec_for_a_stale_lease_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let agent = Agent::with_identity(dir.path().join("actor-id")).await?;
    agent.bind("01J0", None).await?;
    let frames = agent
        .exec_frames("01J-STALE", &["/bin/sh", "-c", "touch /tmp/leaked"])
        .await?;
    assert!(
        matches!(
            frames.as_slice(),
            [ExecFrame::Response(RunCommandResponse::Denied(
                LeaseError::LeaseNotFound { .. }
            ))]
        ),
        "got {frames:?}"
    );
    Ok(())
}

#[tokio::test]
async fn files_write_and_read_round_trip_over_http() -> TestResult {
    let dir = tempfile::tempdir()?;
    let workspace = tempfile::tempdir()?;
    let agent = Agent::with_identity(dir.path().join("actor-id")).await?;
    agent
        .bind("01J0", Some(workspace.path().to_string_lossy().to_string()))
        .await?;
    let write = json!({"lease_id": "01J0", "files": [{"path": "note.txt", "content": b"written".to_vec()}]});
    assert_eq!(
        agent.answer::<WriteFilesResponse>(FILES_WRITE_PATH, &write).await?,
        WriteFilesResponse::Written
    );
    let read = json!({"lease_id": "01J0", "path": "note.txt"});
    let content = agent.answer::<ReadFileResponse>(FILES_READ_PATH, &read).await?;
    assert_eq!(
        content,
        ReadFileResponse::Content {
            data: b"written".to_vec()
        }
    );
    Ok(())
}

fn user(lease_id: &str) -> Value {
    json!({"lease_id": lease_id, "username": "builder"})
}

fn group(lease_id: &str) -> Value {
    json!({"lease_id": lease_id, "groupname": "agents"})
}

fn membership(lease_id: &str) -> Value {
    json!({"lease_id": lease_id, "username": "builder", "groupname": "agents", "action": "add"})
}

#[tokio::test]
async fn user_provisioning_for_a_stale_lease_is_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let agent = Agent::with_identity(dir.path().join("actor-id")).await?;
    agent.bind("01J0", None).await?;
    let created: CreateSandboxUserResponse = agent.answer(USERS_CREATE_PATH, &user("01J-STALE")).await?;
    assert!(
        matches!(
            created,
            CreateSandboxUserResponse::Denied(LeaseError::LeaseNotFound { .. })
        ),
        "got {created:?}"
    );
    let grouped: CreateSandboxGroupResponse = agent.answer(GROUPS_CREATE_PATH, &group("01J-STALE")).await?;
    assert!(
        matches!(
            grouped,
            CreateSandboxGroupResponse::Denied(LeaseError::LeaseNotFound { .. })
        ),
        "got {grouped:?}"
    );
    let modified: ModifyGroupMembershipResponse = agent.answer(GROUP_MEMBERSHIP_PATH, &membership("01J-STALE")).await?;
    assert!(
        matches!(
            modified,
            ModifyGroupMembershipResponse::Denied(LeaseError::LeaseNotFound { .. })
        ),
        "got {modified:?}"
    );
    Ok(())
}

/// A bound lease reaches the provisioning itself. What that answers depends on the image and on whether the agent is
/// root, so the assertion is only that it is not the lease refusal.
#[tokio::test]
async fn provisioning_for_the_bound_lease_reaches_the_implementation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let agent = Agent::with_identity(dir.path().join("actor-id")).await?;
    agent.bind("01J0", None).await?;
    let created: CreateSandboxUserResponse = agent.answer(USERS_CREATE_PATH, &user("01J0")).await?;
    assert!(
        !matches!(
            created,
            CreateSandboxUserResponse::Denied(LeaseError::LeaseNotFound { .. })
        ),
        "got {created:?}"
    );
    let grouped: CreateSandboxGroupResponse = agent.answer(GROUPS_CREATE_PATH, &group("01J0")).await?;
    assert!(
        !matches!(
            grouped,
            CreateSandboxGroupResponse::Denied(LeaseError::LeaseNotFound { .. })
        ),
        "got {grouped:?}"
    );
    let modified: ModifyGroupMembershipResponse = agent.answer(GROUP_MEMBERSHIP_PATH, &membership("01J0")).await?;
    assert!(
        !matches!(
            modified,
            ModifyGroupMembershipResponse::Denied(LeaseError::LeaseNotFound { .. })
        ),
        "got {modified:?}"
    );
    Ok(())
}

#[tokio::test]
async fn identity_is_read_fresh_on_every_request() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("actor-id");
    let agent = Agent::with_identity(path.clone()).await?;
    assert_eq!(agent.get(IDENTITY_PATH).await?.status(), 503);
    tokio::fs::write(&path, "actor-first\n").await?;
    let first: ActorIdentity = agent.get(IDENTITY_PATH).await?.json().await?;
    assert_eq!(first.actor_id.as_str(), "actor-first");
    tokio::fs::write(&path, "actor-second\n").await?;
    let second: ActorIdentity = agent.get(IDENTITY_PATH).await?.json().await?;
    assert_eq!(second.actor_id.as_str(), "actor-second");
    Ok(())
}

#[tokio::test]
async fn unmatched_paths_are_not_found_when_no_port_is_exposed() -> TestResult {
    let dir = tempfile::tempdir()?;
    let agent = Agent::with_identity(dir.path().join("actor-id")).await?;
    agent.bind("01J0", None).await?;
    assert_eq!(agent.get("/anything/else").await?.status(), 404);
    Ok(())
}

#[tokio::test]
async fn unmatched_paths_reverse_proxy_to_the_exposed_port() -> TestResult {
    let dir = tempfile::tempdir()?;
    let agent = Agent::with_identity(dir.path().join("actor-id")).await?;
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = upstream.local_addr()?.port();
    let app = axum::Router::new().route("/hello", axum::routing::get(|| async { "from the workload" }));
    tokio::spawn(serve(upstream, app, agent.cancel.clone()));
    let body = json!({"lease_id": "01J0", "exposed_port": port});
    assert_eq!(agent.post(BIND_PATH, &body).await?.status(), 204);
    let proxied = agent.get("/hello").await?;
    assert_eq!(proxied.status(), 200);
    assert_eq!(proxied.text().await?, "from the workload");
    Ok(())
}

#[tokio::test]
async fn an_exposed_port_nobody_listens_on_is_a_bad_gateway() -> TestResult {
    let dir = tempfile::tempdir()?;
    let agent = Agent::with_identity(dir.path().join("actor-id")).await?;
    let unused = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = unused.local_addr()?.port();
    drop(unused);
    let body = json!({"lease_id": "01J0", "exposed_port": port});
    assert_eq!(agent.post(BIND_PATH, &body).await?.status(), 204);
    assert_eq!(agent.get("/hello").await?.status(), 502);
    Ok(())
}

async fn agent_requiring(minimum: u32) -> Result<(Agent, tempfile::TempDir), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let state = AgentState::new(ActorIdentitySource::new(dir.path().join("actor-id")))
        .with_min_worker_protocol_version(minimum);
    Ok((Agent::start(state).await?, dir))
}

#[tokio::test]
async fn a_worker_below_the_minimum_is_refused_with_a_typed_error() -> TestResult {
    let (agent, _dir) = agent_requiring(2).await?;
    let url = format!("{}{IDENTITY_PATH}", agent.base);
    let old = agent
        .client
        .get(&url)
        .header(WORKER_PROTOCOL_VERSION_HEADER, "1")
        .send()
        .await?;
    assert_eq!(old.status(), 426);
    let status = old.status().as_u16();
    let failure = GuestCallFailure::from_http(status, &old.bytes().await?);
    assert!(!failure.may_fall_back());
    assert_eq!(
        failure,
        GuestCallFailure::Refused(GuestProtocolError::VersionTooOld {
            peer_version: 1,
            min_supported: 2,
            this_version: GUEST_PROTOCOL_VERSION,
        })
    );
    assert_eq!(agent.client.get(&url).send().await?.status(), 426);
    let current = agent
        .client
        .get(&url)
        .header(WORKER_PROTOCOL_VERSION_HEADER, "2")
        .send()
        .await?;
    assert_ne!(current.status(), 426);
    Ok(())
}

#[tokio::test]
async fn a_malformed_version_header_is_refused() -> TestResult {
    let (agent, _dir) = agent_requiring(1).await?;
    let url = format!("{}{IDENTITY_PATH}", agent.base);
    let response = agent
        .client
        .get(&url)
        .header(WORKER_PROTOCOL_VERSION_HEADER, "abc")
        .send()
        .await?;
    assert_eq!(response.status(), 426);
    let failure = GuestCallFailure::from_http(426, &response.bytes().await?);
    assert_eq!(failure, GuestCallFailure::Refused(GuestProtocolError::VersionMalformed));
    Ok(())
}

#[tokio::test]
async fn the_agent_announces_its_versions_and_readiness_is_never_gated() -> TestResult {
    let (agent, _dir) = agent_requiring(2).await?;
    let ready = agent.client.get(format!("{}{READYZ_PATH}", agent.base)).send().await?;
    assert_eq!(ready.status(), 200);
    assert_eq!(
        ready.headers()[AGENT_PROTOCOL_VERSION_HEADER],
        GUEST_PROTOCOL_VERSION.to_string().as_str()
    );
    assert_eq!(ready.headers()[AGENT_MIN_PROTOCOL_VERSION_HEADER], "2");
    let refused = agent
        .client
        .get(format!("{}{IDENTITY_PATH}", agent.base))
        .send()
        .await?;
    assert_eq!(refused.status(), 426);
    assert_eq!(refused.headers()[AGENT_MIN_PROTOCOL_VERSION_HEADER], "2");
    Ok(())
}

#[tokio::test]
async fn a_default_agent_refuses_a_worker_that_sends_no_header() -> TestResult {
    let dir = tempfile::tempdir()?;
    let agent = Agent::with_identity(dir.path().join("actor-id")).await?;
    let url = format!("{}{IDENTITY_PATH}", agent.base);
    assert_eq!(agent.client.get(&url).send().await?.status(), 426);
    assert_ne!(agent.get(IDENTITY_PATH).await?.status(), 426);
    Ok(())
}

#[tokio::test]
async fn the_override_admits_a_legacy_worker_again() -> TestResult {
    let (agent, _dir) = agent_requiring(0).await?;
    let url = format!("{}{IDENTITY_PATH}", agent.base);
    assert_ne!(agent.client.get(&url).send().await?.status(), 426);
    Ok(())
}

#[tokio::test]
async fn proxied_traffic_is_not_gated_by_the_version_check() -> TestResult {
    let (agent, _dir) = agent_requiring(2).await?;
    let response = agent.client.get(format!("{}/anything", agent.base)).send().await?;
    assert_eq!(response.status(), 404);
    assert!(response.headers().get(AGENT_PROTOCOL_VERSION_HEADER).is_none());
    Ok(())
}

#[test]
fn the_default_minimum_is_the_current_version() {
    assert_eq!(MIN_SUPPORTED_WORKER_PROTOCOL_VERSION, GUEST_PROTOCOL_VERSION);
}
