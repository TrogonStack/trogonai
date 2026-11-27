// Shared by several test binaries, and each one uses only part of it.
#![allow(dead_code, reason = "shared test support; every binary uses a different subset")]

//! The pieces every worker-restart test needs: a worker process in front of a fake native sandbox, a signed
//! targeted lease, the commands that give that lease something to recover, and a management proxy that sits
//! between the worker and the fake provider API.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_nats::jetstream;
use axum::body::{Body, to_bytes};
use axum::extract::{Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::Response;
use base64::Engine;
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::sync::watch;
use trogon_sandbox_contract::{
    CancelCommandLaunchRequest, CancelCommandLaunchResponse, CancelIntent, DispatchAttemptId, DispatchTarget,
    ExecutionId, GetSandboxDomainRequest, GetSandboxDomainResponse, LaunchKey, LeaseAttemptIdentity, LeaseEpoch,
    LeaseId, Port, ProviderCleanupRequest, ProviderCleanupResponse, ProviderDispatchPayload, ProviderKind,
    ProviderLease, RunCommandRequest, RunCommandResponse, WorkerDispatchProtocol, WorkerHeartbeat,
};
use trogon_sandbox_protocol::{DispatchSigner, EpochMode, VerbScope, subjects};
use url::Url;

use crate::authority_support;
use crate::ensure_support::{Boxed, Context};
use crate::native_access_support::NativeAccess;
use crate::native_command_support::{Binding, Guest, NativeEnvd};
use crate::native_process_support::{Api, Control, SANDBOX_ID};
use crate::worker_process_support::{self, Nats, Process, signed};

/// The lease epoch every restart test dispatches under.
pub const EPOCH: u64 = 7;

/// Everything a restart test stands up around the worker, except the worker itself.
pub struct Fixture {
    pub nats: Nats,
    pub context: jetstream::Context,
    pub guest: Arc<Guest>,
    pub envd: NativeEnvd,
    pub api: Api,
    pub native: NativeAccess,
    pub ledger: PathBuf,
    pub signer: DispatchSigner,
    _directory: tempfile::TempDir,
}

impl Fixture {
    pub async fn start(key_id: &str, seed: u8) -> Result<Self, Boxed> {
        let nats = Nats::start().await;
        let context = jetstream::new(nats.client.clone());
        let guest = Guest::create()?;
        let envd = NativeEnvd::start(Arc::clone(&guest)).await?;
        let api = Api::start(Arc::clone(&guest)).await?;
        let native = NativeAccess::start(envd.url.clone(), Arc::clone(&api.control)).await?;
        let directory = tempfile::tempdir()?;
        Ok(Self {
            nats,
            context,
            guest,
            envd,
            api,
            native,
            ledger: directory.path().join("dispatch.sqlite"),
            signer: DispatchSigner::new(key_id, [seed; 32]),
            _directory: directory,
        })
    }

    /// Starts a worker against `api_url` and `envd_url` and returns it with its heartbeat feed.
    pub async fn spawn(&self, api_url: &Url, envd_url: &Url) -> Result<(Process, async_nats::Subscriber), Boxed> {
        let heartbeats = self
            .nats
            .client
            .subscribe(subjects::worker_heartbeat_filter(ProviderKind::E2b))
            .await?;
        self.nats.client.flush().await?;
        let worker = worker_process_support::worker_with_epoch_mode(
            &self.nats.url,
            api_url,
            envd_url,
            &self.ledger,
            &self.signer,
            10,
            EpochMode::Require,
        )?;
        Ok((worker, heartbeats))
    }

    /// Starts a worker and waits for the heartbeat that says it is ready.
    pub async fn boot(&self, api_url: &Url, envd_url: &Url) -> Result<(Process, WorkerHeartbeat), Boxed> {
        let (worker, mut heartbeats) = self.spawn(api_url, envd_url).await?;
        let heartbeat = tokio::time::timeout(Duration::from_secs(15), heartbeats.next())
            .await?
            .context("the worker did not become ready")?;
        Ok((worker, serde_json::from_slice(&heartbeat.payload)?))
    }

    /// Stops the fake servers and removes whatever the guest still runs unless the product already did.
    pub async fn finish(self) -> Result<(), Boxed> {
        let product_removed = self.api.control.product_removed.load(Ordering::SeqCst);
        self.native.shutdown().await;
        self.api.shutdown().await;
        self.envd.shutdown().await;
        if !product_removed {
            self.guest.cleanup()?;
        }
        Ok(())
    }
}

/// One signed targeted lease: who may use it, and the dispatch that creates it.
pub struct Lease {
    pub id: LeaseId,
    pub target: DispatchTarget,
    pub scope: VerbScope,
    pub dispatch: ProviderDispatchPayload,
}

impl Lease {
    pub fn new(heartbeat: &WorkerHeartbeat, name: &str, ports: &[u16]) -> Result<Self, Boxed> {
        let target = DispatchTarget {
            worker_id: heartbeat.worker_id.clone(),
            attempt_id: DispatchAttemptId::new(format!("{name}-attempt"))?,
            epoch: Some(LeaseEpoch::new(EPOCH)?),
        };
        let id = LeaseId::new(format!("{name}-lease"));
        let mut request = authority_support::lease_request()?;
        request.ttl_seconds = 600;
        let mut parts = request.spec.into_parts();
        parts.exposed_ports = ports.iter().map(|port| Port::new(*port)).collect::<Result<_, _>>()?;
        request.spec = trogon_sandbox_contract::SandboxSpec::new(parts)?;
        let scope = VerbScope {
            tenant: request.tenant.clone(),
            lease_id: id.clone(),
            worker_id: Some(target.worker_id.clone()),
            attempt_id: Some(target.attempt_id.clone()),
            epoch: target.epoch,
        };
        let dispatch = ProviderDispatchPayload {
            lease_id: id.clone(),
            request,
            correlation_id: None,
            resolved_drives: vec![],
            target: Some(target.clone()),
        };
        Ok(Self {
            id,
            target,
            scope,
            dispatch,
        })
    }

    pub fn dispatch_subject(&self) -> String {
        subjects::worker_dispatch(ProviderKind::E2b, &self.target.worker_id)
    }

    pub fn run_subject(&self) -> String {
        subjects::provider_run_command(ProviderKind::E2b, &self.id)
    }

    pub fn cancel_subject(&self) -> String {
        subjects::provider_cancel_command_launch(ProviderKind::E2b, &self.id)
    }

    /// The custody key of a strong launch of this lease.
    pub fn command_key(&self, launch_key: &str) -> Result<String, Boxed> {
        Ok(trogon_sandbox_protocol::tenant_scoped_kv_key(
            &self.dispatch.request.tenant,
            &serde_json::to_string(&(self.id.as_str(), launch_key))?,
        ))
    }

    /// The key of this lease's assignment record.
    pub fn assignment_key(&self) -> Result<String, Boxed> {
        Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&(&self.target.worker_id, &self.id))?))
    }

    /// Dispatches the lease and checks the grant binds exactly this assignment.
    pub async fn grant(&self, fixture: &Fixture) -> Result<ProviderLease, Boxed> {
        let grant: ProviderLease =
            worker_process_support::dispatch(&fixture.nats, &fixture.signer, &self.dispatch, &self.target.worker_id)
                .await?;
        assert!(
            grant.sandbox_id.as_str() == SANDBOX_ID
                && grant.worker_id.as_ref() == Some(&self.target.worker_id)
                && grant.attempt_id.as_ref() == Some(&self.target.attempt_id)
                && grant.epoch == self.target.epoch,
            "the grant must bind the exact native assignment"
        );
        Ok(grant)
    }

    pub async fn request<T: serde::Serialize, R: serde::de::DeserializeOwned>(
        &self,
        fixture: &Fixture,
        subject: String,
        request: &T,
    ) -> Result<R, Boxed> {
        signed(&fixture.nats, &fixture.signer, subject, request, &self.scope).await
    }

    /// Starts an ordinary detached command, which gives the lease a native process group no launch key owns.
    pub async fn sentinel(&self, fixture: &Fixture) -> Result<Binding, Boxed> {
        let request: RunCommandRequest = serde_json::from_value(json!({
            "lease_id": self.id, "command": ["sleep", "300"], "detached": true, "deadline": "lease", "run_as_user": "root",
        }))?;
        let response: RunCommandResponse = self.request(fixture, self.run_subject(), &request).await?;
        assert!(
            matches!(response, RunCommandResponse::Started(_)),
            "the native sentinel positive control must start: {response:?}"
        );
        fixture.guest.last_binding()
    }

    /// The strong launch the restart tests recover: it writes `effect` once and then waits.
    pub fn strong(&self, fixture: &Fixture, launch_key: &str, effect: &str) -> Result<RunCommandRequest, Boxed> {
        let script = format!(
            "echo owned >>{}; sleep 300 & wait",
            fixture.guest.path(effect).display()
        );
        Ok(serde_json::from_value(json!({
            "lease_id": self.id, "launch_key": launch_key, "require_launch_cancellation": true,
            "command": ["bash", "-c", script], "detached": true, "timeout_seconds": 180, "run_as_user": "root",
        }))?)
    }

    /// Runs the strong launch and waits until its user effect is visible.
    pub async fn start_strong(
        &self,
        fixture: &Fixture,
        request: &RunCommandRequest,
        effect: &str,
    ) -> Result<(ExecutionId, Binding), Boxed> {
        let response: RunCommandResponse = self.request(fixture, self.run_subject(), request).await?;
        let RunCommandResponse::Started(started) = response else {
            return Err(format!("the strong native positive control must start: {response:?}").into());
        };
        let binding = fixture.guest.last_binding()?;
        tokio::time::timeout(Duration::from_secs(5), async {
            while fixture.guest.effect(effect)?.as_deref() != Some(b"owned\n".as_slice()) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            Ok::<_, Boxed>(())
        })
        .await
        .context("the real positive native effect did not become observable")??;
        Ok((started.execution_id, binding))
    }

    pub async fn cancel_launch(
        &self,
        fixture: &Fixture,
        launch_key: &str,
    ) -> Result<CancelCommandLaunchResponse, Boxed> {
        let request = CancelCommandLaunchRequest {
            lease_id: self.id.clone(),
            launch_key: LaunchKey::new(launch_key)?,
        };
        self.request(fixture, self.cancel_subject(), &request).await
    }

    /// The cleanup request that releases this exact targeted lease.
    pub async fn release(&self, fixture: &Fixture) -> Result<ProviderCleanupResponse, Boxed> {
        let request = ProviderCleanupRequest {
            identity: LeaseAttemptIdentity {
                lease_id: self.id.clone(),
                provider: ProviderKind::E2b,
                target: self.target.clone(),
            },
            intent: CancelIntent::Release,
        };
        self.request(
            fixture,
            subjects::worker_cleanup(ProviderKind::E2b, &self.target.worker_id),
            &request,
        )
        .await
    }
}

/// What a healthy worker advertises: epoch support and targeted dispatch.
pub fn assert_targeted(heartbeat: &WorkerHeartbeat) {
    assert!(
        heartbeat.supports_lease_epoch.is_supported()
            && heartbeat.dispatch_protocol == WorkerDispatchProtocol::TargetedV1,
        "the worker must advertise observed epoch support and targeted dispatch"
    );
}

/// The native call counters, in the order creates, deletes, connects, lists, gets, foreign gets, foreign deletes,
/// pauses.
pub fn counters(control: &Control) -> [usize; 8] {
    [
        &control.creates,
        &control.deletes,
        &control.connects,
        &control.lists,
        &control.gets,
        &control.foreign_gets,
        &control.foreign_deletes,
        &control.pauses,
    ]
    .map(|counter| counter.load(Ordering::SeqCst))
}

/// Both native groups are alive, and the user effect ran exactly once.
pub fn native_controls(fixture: &Fixture, effect: &str, sentinel: Binding, target: Binding) -> Result<(), Boxed> {
    assert!(
        !fixture.guest.group_members(sentinel)?.is_empty()
            && !fixture.guest.group_members(target)?.is_empty()
            && fixture.guest.bootstrap_calls.load(Ordering::SeqCst) == 2,
        "recovery must preserve both exact native groups without a new bootstrap"
    );
    assert_eq!(
        fixture.guest.effect(effect)?.as_deref(),
        Some(b"owned\n".as_slice()),
        "recovery must retain one original native user effect"
    );
    Ok(())
}

/// A key-value bucket as the tests compare it: the stream position and every live value.
#[derive(Eq, PartialEq)]
pub struct Snapshot {
    sequence: u64,
    pub values: BTreeMap<String, Vec<u8>>,
}

pub async fn snapshot(context: &jetstream::Context, bucket: &str) -> Result<Snapshot, Boxed> {
    let store = context.get_key_value(bucket).await?;
    let mut native_stream = store.stream.clone();
    let sequence = native_stream.info().await?.state.last_sequence;
    let mut keys = store.keys().await?;
    let mut values = BTreeMap::new();
    while let Some(key) = keys.next().await {
        let key = key?;
        let value = store.get(&key).await?.context("retained authority disappeared")?;
        values.insert(key, value.to_vec());
    }
    assert_eq!(
        native_stream.info().await?.state.last_sequence,
        sequence,
        "authority changed during its exact observation"
    );
    Ok(Snapshot { sequence, values })
}

/// Waits for the worker to exit and checks it never reported ready.
pub async fn refuses(worker: &mut Process, heartbeats: &mut async_nats::Subscriber) -> Result<(), Boxed> {
    tokio::time::timeout(Duration::from_secs(20), async {
        while worker.is_alive()? {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok::<(), Boxed>(())
    })
    .await
    .context("the worker neither refused startup nor settled")??;
    assert!(
        tokio::time::timeout(Duration::from_millis(200), heartbeats.next())
            .await
            .is_err(),
        "the refused worker must not publish readiness"
    );
    Ok(())
}

/// The part of a management exchange a test may rewrite.
#[derive(Clone, Copy, Debug)]
pub enum Hop {
    Create,
    Inspect,
}

/// Rewrites one management response given its request body; `None` rejects the request.
pub type Rewrite = Arc<dyn Fn(Hop, &Value, &mut Value) -> Option<()> + Send + Sync>;

#[derive(Clone)]
struct Forward {
    upstream: String,
    client: reqwest::Client,
    rewrite: Rewrite,
    creates: Arc<AtomicUsize>,
    inspections: Arc<AtomicUsize>,
    created: watch::Sender<bool>,
    release: watch::Receiver<bool>,
}

async fn forward(State(mut state): State<Forward>, request: Request) -> Result<Response, StatusCode> {
    let hop = if request.method() == Method::POST && request.uri().path() == "/v2/sandboxes" {
        Some(Hop::Create)
    } else if request.method() == Method::GET && request.uri().path() == format!("/sandboxes/{SANDBOX_ID}") {
        Some(Hop::Inspect)
    } else {
        None
    };
    let (mut parts, body) = request.into_parts();
    let body = to_bytes(body, 128 * 1024).await.map_err(|_| StatusCode::BAD_REQUEST)?;
    let request_body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    parts.headers.remove(header::HOST);
    let upstream = state
        .client
        .request(parts.method, format!("{}{}", state.upstream, parts.uri))
        .headers(parts.headers)
        .body(body)
        .send()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    let status = upstream.status();
    let mut headers = upstream.headers().clone();
    headers.remove(header::TRANSFER_ENCODING);
    headers.remove(header::CONTENT_LENGTH);
    let mut body = upstream.bytes().await.map_err(|_| StatusCode::BAD_GATEWAY)?;
    if let Some(hop) = hop.filter(|_| status.is_success()) {
        let mut value: Value = serde_json::from_slice(&body).map_err(|_| StatusCode::BAD_GATEWAY)?;
        (value["sandboxID"] == SANDBOX_ID)
            .then_some(())
            .ok_or(StatusCode::BAD_GATEWAY)?;
        (state.rewrite)(hop, &request_body, &mut value).ok_or(StatusCode::BAD_REQUEST)?;
        body = serde_json::to_vec(&value).map_err(|_| StatusCode::BAD_GATEWAY)?.into();
        match hop {
            Hop::Create => {
                state.creates.fetch_add(1, Ordering::SeqCst);
                state.created.send_replace(true);
                tokio::time::timeout(Duration::from_secs(30), state.release.wait_for(|ready| *ready))
                    .await
                    .map_err(|_| StatusCode::GATEWAY_TIMEOUT)?
                    .map_err(|_| StatusCode::BAD_GATEWAY)?;
            }
            Hop::Inspect => {
                state.inspections.fetch_add(1, Ordering::SeqCst);
            }
        }
    }
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = status;
    *response.headers_mut() = headers;
    Ok(response)
}

/// A forwarding proxy in front of the fake provider API. It rewrites create and inspect responses, and can hold
/// the create response back until the test releases it.
pub struct Facade {
    pub url: Url,
    pub creates: Arc<AtomicUsize>,
    pub inspections: Arc<AtomicUsize>,
    created: watch::Receiver<bool>,
    release: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Facade {
    pub async fn start(upstream: &Url, rewrite: Rewrite, hold_create: bool) -> Result<Self, Boxed> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let url = Url::parse(&format!("http://{}", listener.local_addr()?))?;
        let creates = Arc::new(AtomicUsize::new(0));
        let inspections = Arc::new(AtomicUsize::new(0));
        let (created_tx, created) = watch::channel(false);
        let (release, release_rx) = watch::channel(!hold_create);
        let state = Forward {
            upstream: upstream.as_str().trim_end_matches('/').to_owned(),
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(10))
                .build()?,
            rewrite,
            creates: Arc::clone(&creates),
            inspections: Arc::clone(&inspections),
            created: created_tx,
            release: release_rx,
        };
        let app = axum::Router::new().fallback(forward).with_state(state);
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Ok(Self {
            url,
            creates,
            inspections,
            created,
            release,
            task: Some(task),
        })
    }

    /// Waits until the upstream create has answered; the held response is still unsent.
    pub async fn wait_created(&mut self) -> Result<(), Boxed> {
        tokio::time::timeout(Duration::from_secs(10), self.created.wait_for(|created| *created))
            .await
            .context("the exact upstream native create did not complete")??;
        Ok(())
    }

    /// Lets the held create response through.
    pub fn release(&self) {
        self.release.send_replace(true);
    }

    pub async fn shutdown(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for Facade {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

/// What a preview-recovery run observed: the domain answer before the restart and the one after it.
pub struct Preview {
    pub fresh: GetSandboxDomainResponse,
    pub recovered: GetSandboxDomainResponse,
}

/// Whether `token` appears in any retained authority snapshot or host journal file.
fn leaks(directory: &std::path::Path, snapshots: &[&Snapshot], token: &str) -> Result<bool, Boxed> {
    let contains = |bytes: &[u8]| bytes.windows(token.len()).any(|window| window == token.as_bytes());
    let mut found = snapshots
        .iter()
        .any(|snapshot| snapshot.values.values().any(|bytes| contains(bytes)));
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            found |= contains(&std::fs::read(entry.path())?);
        }
    }
    Ok(found)
}

/// Runs a lease with an exposed port through a worker restart while `rewrite` shapes the management answers,
/// and reports the preview the worker offered before and after. `token` is a traffic credential that must
/// never reach retained authority.
pub async fn preview_scenario(name: &str, rewrite: Rewrite, token: Option<&str>) -> Result<Preview, Boxed> {
    const PORT: u16 = 3000;
    let effect = format!("{name}-effect");
    let launch_key = format!("{name}-owned-start");
    let fixture = Fixture::start(&format!("{name}-fixture"), 83).await?;
    let facade = Facade::start(&fixture.api.url, rewrite, false).await?;
    let (mut worker, heartbeat) = fixture.boot(&facade.url, &fixture.native.url).await?;
    assert_targeted(&heartbeat);
    let lease = Lease::new(&heartbeat, name, &[PORT])?;
    lease.grant(&fixture).await?;
    assert_eq!(
        facade.creates.load(Ordering::SeqCst),
        1,
        "the fresh project response must accompany an exact targeted native grant"
    );
    let domain = GetSandboxDomainRequest {
        lease_id: lease.id.clone(),
        port: PORT,
        wait_ms: None,
    };
    let domain_subject = subjects::provider_get_domain(ProviderKind::E2b, &lease.id);
    let fresh: GetSandboxDomainResponse = lease.request(&fixture, domain_subject.clone(), &domain).await?;
    let sentinel = lease.sentinel(&fixture).await?;
    let request = lease.strong(&fixture, &launch_key, &effect)?;
    let (execution_id, target) = lease.start_strong(&fixture, &request, &effect).await?;
    assert!(
        sentinel.pid != target.pid,
        "the native controls need independent identities"
    );
    native_controls(&fixture, &effect, sentinel, target)?;
    let assignments = snapshot(&fixture.context, "sandbox-e2b-assignments").await?;
    let commands = snapshot(&fixture.context, "sandbox-e2b-command-custody").await?;
    assert!(
        !assignments.values.is_empty() && !commands.values.is_empty(),
        "the positive retained authority must be nonempty"
    );
    let directory = fixture.ledger.parent().context("the ledger has no directory")?;
    let fence = fixture.ledger.with_extension("fence");
    if let Some(token) = token {
        assert!(
            !leaks(directory, &[&assignments, &commands], token)?,
            "the traffic credential must not enter plaintext authority or journal files"
        );
    }
    let producer = worker.id();
    worker.stop()?;
    let original_fence = std::fs::read(&fence)?;
    fixture.api.control.recovered_access.store(true, Ordering::SeqCst);
    let (mut worker, recovered_heartbeat) = fixture.boot(&facade.url, &fixture.native.url).await?;
    assert_targeted(&recovered_heartbeat);
    assert!(
        worker.is_alive()?
            && worker.id() != producer
            && recovered_heartbeat.worker_id == lease.target.worker_id
            && recovered_heartbeat.capacity.current_leases == 1
            && facade.inspections.load(Ordering::SeqCst) > 0,
        "recovery needs occupied native adoption through the read-only management answer"
    );
    let replay: RunCommandResponse = lease.request(&fixture, lease.run_subject(), &request).await?;
    assert!(
        matches!(&replay, RunCommandResponse::Started(started) if started.execution_id == execution_id),
        "a native command replay must work independently of preview recovery"
    );
    let recovered: GetSandboxDomainResponse = lease.request(&fixture, domain_subject, &domain).await?;
    let counts = counters(&fixture.api.control);
    assert!(
        counts[0] == 1
            && counts[1] == 0
            && counts[2] == 0
            && counts[3] == 0
            && counts[4] > 0
            && counts[5] == 0
            && counts[6] == 0
            && counts[7] == 0,
        "recovery must use exact read-only native access without create, connect, delete, list or foreign fallback: {counts:?}"
    );
    let after_assignments = snapshot(&fixture.context, "sandbox-e2b-assignments").await?;
    let after_commands = snapshot(&fixture.context, "sandbox-e2b-command-custody").await?;
    assert!(
        assignments == after_assignments && commands == after_commands && original_fence == std::fs::read(&fence)?,
        "preview management must preserve the original native and command authority"
    );
    if let Some(token) = token {
        assert!(
            !leaks(directory, &[&after_assignments, &after_commands], token)?,
            "the traffic credential must not enter plaintext authority after recovery"
        );
    }
    native_controls(&fixture, &effect, sentinel, target)?;
    let canceled = lease.cancel_launch(&fixture, &launch_key).await?;
    let again = lease.cancel_launch(&fixture, &launch_key).await?;
    assert!(
        canceled
            == CancelCommandLaunchResponse::Canceled {
                execution_id: Some(execution_id)
            }
            && again == canceled
            && fixture.guest.group_members(target)?.is_empty()
            && !fixture.guest.group_members(sentinel)?.is_empty()
            && fixture.guest.bootstrap_calls.load(Ordering::SeqCst) == 2
            && fixture.native.recovered.load(Ordering::SeqCst) > 0
            && fixture.native.rejected.load(Ordering::SeqCst) == 0,
        "the preview answer must not disturb fresh-native cancellation, its replay or the sentinel"
    );
    let released = lease.release(&fixture).await?;
    worker.stop()?;
    let counts = counters(&fixture.api.control);
    assert!(
        matches!(released, ProviderCleanupResponse::Cleaned { .. })
            && fixture.api.control.product_removed.load(Ordering::SeqCst)
            && counts[0] == 1
            && counts[1] == 1
            && counts[2] == 0
            && counts[3] == 0
            && counts[4] > 0
            && counts[5] == 0
            && counts[6] == 0
            && counts[7] == 0,
        "the original targeted authority must earn physical native cleanup: {counts:?}"
    );
    facade.shutdown().await;
    fixture.finish().await?;
    Ok(Preview { fresh, recovered })
}
