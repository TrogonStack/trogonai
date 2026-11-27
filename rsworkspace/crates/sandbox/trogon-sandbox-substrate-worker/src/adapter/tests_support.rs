//! Shared fixtures for the adapter, fleet and worker tests.

use std::collections::BTreeMap;
use std::error::Error;
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Notify;
use trogon_sandbox_contract::{
    LeaseName, LeasePersistence, LeasePrewarm, LeaseRequest, LeaseRequestKey, OciImageRef, ProviderKind, SandboxImage,
    SandboxSpec, SandboxSpecParts, SandboxTags, SubstrateSizeClasses, TenantId, WorkspaceSource,
};
use trogon_std::env::InMemoryEnv;

use super::SubstrateProvider;
use crate::actor_index::{ActorEntry, ActorIndex, ActorIndexError, ActorIndexResult};
use crate::ate_client::ResourceRevision;
use crate::ate_client::{AteError, AteResult, Control};
use crate::config::{DEFAULT_ROUTER_ENDPOINT, SandboxClass, SubstrateConfig};
use crate::proto;
use crate::template::{TemplateIndex, TemplateIndexError, TemplateRecord, TemplateRegistry};

pub(crate) type TestResult = Result<(), Box<dyn Error>>;

/// A tenant image pinned by digest, so the template build accepts it.
pub(crate) fn app_image() -> &'static str {
    "registry.example/app@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
}

pub(crate) fn registry_image() -> SandboxImage {
    SandboxImage::Registry {
        reference: OciImageRef::parse_unqualified(app_image()),
        pull_secret: None,
    }
}

/// The fields of a spec this provider accepts, for a test to change before it builds the spec.
pub(crate) fn spec_parts() -> Result<SandboxSpecParts, Box<dyn Error>> {
    Ok(SandboxSpecParts {
        image: registry_image(),
        workspace: WorkspaceSource::empty(),
        env: BTreeMap::new(),
        cpu_seconds: NonZeroU32::new(60).ok_or("cpu seconds")?,
        memory_mib: NonZeroU32::new(2048).ok_or("memory")?,
        snapshot_out: None,
        network_policy: None,
        mounts: Vec::new(),
        drives: Vec::new(),
        exposed_ports: Vec::new(),
        boot_mode: Default::default(),
        working_dir: None,
        user: None,
        privileged: Default::default(),
        wasi_capabilities: None,
        fs_policy: None,
        secret_bindings: Default::default(),
        platform: Default::default(),
        idle_timeout_seconds: None,
        no_ttl: Default::default(),
        idle_policy: None,
        vcpu_millis: None,
        disk_mib: None,
        retention: Default::default(),
    })
}

pub(crate) fn spec() -> Result<SandboxSpec, Box<dyn Error>> {
    Ok(SandboxSpec::new(spec_parts()?)?)
}

/// A request for `tenant-a`, anonymous when `name` is `None`.
pub(crate) fn lease_request(name: Option<&str>, persistent: bool) -> Result<LeaseRequest, Box<dyn Error>> {
    Ok(LeaseRequest {
        operation_id: None,
        tenant: TenantId::new("tenant-a")?,
        provider_pref: vec![ProviderKind::Substrate],
        spec: spec()?,
        ttl_seconds: 600,
        idempotency_key: LeaseRequestKey::new("k")?,
        isolation_tier: None,
        host_imports: Vec::new(),
        required_capabilities: None,
        prewarm: LeasePrewarm::Regular,
        tags: SandboxTags::default(),
        name: name.map(LeaseName::new).transpose()?,
        persistent: LeasePersistence::from(persistent),
        native: None,
        region: None,
        fallback_regions: Vec::new(),
    })
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A control plane whose actor walks a scripted status sequence.
pub(crate) struct FakeControl {
    gets: AtomicUsize,
    statuses: Mutex<Vec<proto::ActorState>>,
    create_actor_result: Mutex<Option<AteError>>,
    atespace_error: Mutex<Option<AteError>>,
    existing_template: Mutex<Option<String>>,
    existing_template_space: Mutex<Option<String>>,
    snapshot_state: Mutex<Option<(i32, Vec<proto::Snapshot>)>>,
    pub(crate) snapshot_observed: Notify,
    get_error: Mutex<Option<AteError>>,
    created_name: Mutex<Option<String>>,
    suspend_error: Mutex<Option<AteError>>,
    delete_error: Mutex<Option<AteError>>,
    uid: Mutex<Option<String>>,
    transitions: bool,
    deleted: AtomicBool,
    listed: Mutex<Vec<String>>,
    suspends: AtomicUsize,
    deletes: AtomicUsize,
    resumes: AtomicUsize,
}

impl FakeControl {
    pub(crate) fn running() -> Self {
        Self::with_statuses(vec![proto::ActorState::Running])
    }

    pub(crate) fn with_statuses(statuses: Vec<proto::ActorState>) -> Self {
        Self {
            gets: AtomicUsize::new(0),
            statuses: Mutex::new(statuses),
            create_actor_result: Mutex::new(None),
            atespace_error: Mutex::new(None),
            existing_template: Mutex::new(None),
            existing_template_space: Mutex::new(None),
            snapshot_state: Mutex::new(None),
            snapshot_observed: Notify::new(),
            get_error: Mutex::new(None),
            created_name: Mutex::new(None),
            suspend_error: Mutex::new(None),
            delete_error: Mutex::new(None),
            uid: Mutex::new(None),
            transitions: false,
            deleted: AtomicBool::new(false),
            listed: Mutex::new(Vec::new()),
            suspends: AtomicUsize::new(0),
            deletes: AtomicUsize::new(0),
            resumes: AtomicUsize::new(0),
        }
    }

    pub(crate) fn fail_get_with(&self, error: AteError) {
        *locked(&self.get_error) = Some(error);
    }

    /// Makes `CreateActor` answer with an actor of another name.
    pub(crate) fn rename_created_actor(&self, name: &str) {
        *locked(&self.created_name) = Some(name.to_string());
    }

    pub(crate) fn fail_suspend_with(&self, error: AteError) {
        *locked(&self.suspend_error) = Some(error);
    }

    pub(crate) fn fail_delete_with(&self, error: AteError) {
        *locked(&self.delete_error) = Some(error);
    }

    pub(crate) fn fail_atespace_with(&self, error: AteError) {
        *locked(&self.atespace_error) = Some(error);
    }

    pub(crate) fn fail_create_with(&self, error: AteError) {
        *locked(&self.create_actor_result) = Some(error);
    }

    pub(crate) fn set_existing_template(&self, template: &str) {
        *locked(&self.existing_template) = Some(template.to_string());
    }

    pub(crate) fn set_existing_template_space(&self, atespace: &str) {
        *locked(&self.existing_template_space) = Some(atespace.to_string());
    }

    pub(crate) fn set_snapshot_state(&self, generation: i32, snapshots: Vec<proto::Snapshot>) {
        *locked(&self.snapshot_state) = Some((generation, snapshots));
    }

    pub(crate) fn with_uid(self, uid: &str) -> Self {
        self.set_uid(uid);
        self
    }

    pub(crate) fn with_transitions(mut self) -> Self {
        self.transitions = true;
        self
    }

    pub(crate) fn set_uid(&self, uid: &str) {
        *locked(&self.uid) = Some(uid.to_string());
    }

    pub(crate) fn set_statuses(&self, statuses: Vec<proto::ActorState>) {
        *locked(&self.statuses) = statuses;
    }

    pub(crate) fn set_listed(&self, names: Vec<String>) {
        *locked(&self.listed) = names;
    }

    pub(crate) fn get_count(&self) -> usize {
        self.gets.load(Ordering::SeqCst)
    }

    pub(crate) fn delete_count(&self) -> usize {
        self.deletes.load(Ordering::SeqCst)
    }

    pub(crate) fn suspend_count(&self) -> usize {
        self.suspends.load(Ordering::SeqCst)
    }

    /// How many times `ResumeActor` was called. `ResumeActor` carries no boot/warm flag, so a
    /// call count is all a caller can observe about it.
    pub(crate) fn resume_count(&self) -> usize {
        self.resumes.load(Ordering::SeqCst)
    }

    /// Advances the script. Only `GetActor` does this, so a status sequence reads as the
    /// series of observations a caller makes, independent of how many mutating calls it
    /// takes to get there.
    fn next_status(&self) -> proto::ActorState {
        let mut statuses = locked(&self.statuses);
        if statuses.len() > 1 {
            statuses.remove(0)
        } else {
            statuses.first().copied().unwrap_or(proto::ActorState::Running)
        }
    }

    fn peek_status(&self) -> proto::ActorState {
        locked(&self.statuses)
            .first()
            .copied()
            .unwrap_or(proto::ActorState::Running)
    }

    fn actor_at(&self, actor_ref: proto::ObjectRef, status: proto::ActorState) -> proto::Actor {
        let template = locked(&self.existing_template).clone().unwrap_or_default();
        let snapshot_state = locked(&self.snapshot_state).clone().unwrap_or_else(|| {
            if status == proto::ActorState::Suspended {
                (1, vec![complete_snapshot(1)])
            } else {
                (0, vec![])
            }
        });
        self.snapshot_observed.notify_one();
        proto::Actor {
            metadata: Some(proto::ResourceMetadata {
                atespace: actor_ref.atespace.clone(),
                name: actor_ref.name.clone(),
                uid: locked(&self.uid).clone().unwrap_or_default(),
                version: 1,
                ..Default::default()
            }),
            actor_template: (!template.is_empty()).then_some(proto::ObjectRef {
                atespace: locked(&self.existing_template_space)
                    .clone()
                    .unwrap_or(actor_ref.atespace),
                name: template,
            }),
            status: Some(proto::ActorStatus {
                state: status as i32,
                last_assigned_generation: snapshot_state.0,
                snapshots: snapshot_state.1,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn observed(&self, actor_ref: proto::ObjectRef) -> proto::Actor {
        let status = self.next_status();
        self.actor_at(actor_ref, status)
    }

    fn current(&self, actor_ref: proto::ObjectRef) -> proto::Actor {
        let status = self.peek_status();
        self.actor_at(actor_ref, status)
    }
}

pub(crate) fn complete_snapshot(generation: i32) -> proto::Snapshot {
    proto::Snapshot {
        uuid: format!("native-snapshot-{generation}"),
        generation,
        owner: proto::SnapshotOwner::Actor as i32,
        fidelity: proto::SnapshotFidelity::Memory as i32,
        durable_snapshot: Some(proto::SnapshotStorage {
            state: proto::SnapshotStorageState::Completed as i32,
            object: Some(proto::ObjectSnapshot {
                snapshot_uri: format!("s3://snapshots/native-snapshot-{generation}"),
            }),
        }),
        ..Default::default()
    }
}

#[async_trait]
impl Control for FakeControl {
    async fn create_atespace(&self, atespace: &str) -> AteResult<proto::Atespace> {
        if let Some(error) = locked(&self.atespace_error).clone() {
            return Err(error);
        }
        Ok(proto::Atespace {
            metadata: Some(proto::ResourceMetadata {
                name: atespace.to_string(),
                ..Default::default()
            }),
        })
    }

    async fn get_actor(&self, actor: proto::ObjectRef) -> AteResult<proto::Actor> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        if self.deleted.load(Ordering::SeqCst) {
            return Err(AteError::NotFound {
                message: "the owned actor was deleted".into(),
            });
        }
        if let Some(error) = locked(&self.get_error).clone() {
            return Err(error);
        }
        Ok(self.observed(actor))
    }

    async fn create_actor(&self, mut actor: proto::Actor) -> AteResult<proto::Actor> {
        if let Some(error) = locked(&self.create_actor_result).clone() {
            return Err(error);
        }
        self.deleted.store(false, Ordering::SeqCst);
        if let Some(metadata) = actor.metadata.as_mut() {
            metadata.uid = locked(&self.uid).clone().unwrap_or_default();
            if let Some(template) = actor.actor_template.as_ref() {
                locked(&self.existing_template).get_or_insert_with(|| template.name.clone());
            }
            if let Some(name) = locked(&self.created_name).clone() {
                metadata.name = name;
            }
        }
        Ok(actor)
    }

    async fn resume_actor(&self, actor: proto::ObjectRef) -> AteResult<proto::Actor> {
        self.resumes.fetch_add(1, Ordering::SeqCst);
        if self.transitions {
            self.set_statuses(vec![proto::ActorState::Running]);
        }
        Ok(self.current(actor))
    }

    async fn suspend_actor(&self, actor: proto::ObjectRef) -> AteResult<proto::Actor> {
        self.suspends.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = locked(&self.suspend_error).clone() {
            return Err(error);
        }
        if self.transitions {
            self.set_statuses(vec![proto::ActorState::Suspended]);
        }
        Ok(self.current(actor))
    }

    async fn delete_actor(&self, actor: ResourceRevision) -> AteResult<proto::Actor> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        if let Some(error) = locked(&self.delete_error).clone() {
            return Err(error);
        }
        let actor = self.current(actor.resource().clone());
        self.deleted.store(true, Ordering::SeqCst);
        Ok(actor)
    }

    async fn list_actors(&self, atespace: &str) -> AteResult<Vec<proto::Actor>> {
        let names = locked(&self.listed).clone();
        Ok(names
            .into_iter()
            .map(|name| {
                self.current(proto::ObjectRef {
                    atespace: atespace.to_string(),
                    name,
                })
            })
            .collect())
    }

    async fn create_actor_template(&self, template: proto::ActorTemplate) -> AteResult<proto::ActorTemplate> {
        Ok(template)
    }

    async fn delete_actor_template(&self, _template: ResourceRevision) -> AteResult<()> {
        Ok(())
    }

    async fn get_actor_template(&self, template: proto::ObjectRef) -> AteResult<proto::ActorTemplate> {
        Ok(proto::ActorTemplate {
            metadata: Some(proto::ResourceMetadata {
                atespace: template.atespace.clone(),
                name: template.name.clone(),
                ..Default::default()
            }),
            status: Some(proto::ActorTemplateStatus {
                golden_snapshot_status: Some(proto::GoldenSnapshotStatus {
                    golden_tag: Some(proto::ObjectRef {
                        atespace: template.atespace,
                        name: format!("golden-{}", template.name),
                    }),
                    snapshot_time: None,
                    error_message: String::new(),
                }),
            }),
            ..Default::default()
        })
    }
}

/// A warmed native template whose build claim belongs to another registry.
pub(crate) struct WarmTemplates;

#[async_trait]
impl TemplateIndex for WarmTemplates {
    async fn get(&self, _key: &str) -> Result<Option<TemplateRecord>, TemplateIndexError> {
        Ok(None)
    }

    async fn create(&self, _key: &str, _record: &TemplateRecord) -> Result<bool, TemplateIndexError> {
        Ok(false)
    }

    async fn put(&self, _key: &str, _record: &TemplateRecord) -> Result<(), TemplateIndexError> {
        Ok(())
    }

    async fn delete(&self, _key: &str) -> Result<(), TemplateIndexError> {
        Ok(())
    }
}

pub(crate) fn config() -> Result<SubstrateConfig, Box<dyn Error>> {
    Ok(SubstrateConfig {
        ate_api_endpoint: "http://ate-api-server:8080".to_string(),
        router_endpoint: DEFAULT_ROUTER_ENDPOINT.parse()?,
        agent_image: "registry.example/agent@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
            .to_string(),
        agent_port: 8110,
        sandbox_class: SandboxClass::Gvisor,
        snapshots_location: "gs://bucket/snapshots".to_string(),
        size_classes: "small=2048,medium=8192,large=32768".parse::<SubstrateSizeClasses>()?,
        template_build_timeout: Duration::from_secs(5),
        resume_timeout: Duration::from_millis(200),
        max_concurrent_leases: 64,
        router_tls: None,
        actor_cpu_millicores: 1000,
    })
}

/// A provider over `control` and `index` whose polling is fast enough for a test.
pub(crate) fn provider_with(
    control: Arc<dyn Control>,
    index: Arc<dyn ActorIndex>,
) -> Result<SubstrateProvider, Box<dyn Error>> {
    Ok(provider_over(config()?, control, index)?)
}

/// Like [`provider_with`], for a test that has changed the configuration.
pub(crate) fn provider_over(
    config: SubstrateConfig,
    control: Arc<dyn Control>,
    index: Arc<dyn ActorIndex>,
) -> Result<SubstrateProvider, super::SubstrateError> {
    SubstrateProvider::new(
        config,
        control,
        TemplateRegistry::new(Arc::new(WarmTemplates)).with_poll_interval(Duration::from_millis(1)),
        index,
        &InMemoryEnv::new(),
    )
    .map(|provider| provider.with_poll_interval(Duration::from_millis(1)))
}

/// An ownership index whose storage is down.
pub(crate) struct BrokenIndex;

fn storage_down() -> ActorIndexError {
    ActorIndexError::storage(std::io::Error::other("storage is down"))
}

#[async_trait]
impl ActorIndex for BrokenIndex {
    async fn put(&self, _entry: &ActorEntry) -> ActorIndexResult<()> {
        Err(storage_down())
    }

    async fn get(&self, _atespace: &str, _actor: &str) -> ActorIndexResult<Option<ActorEntry>> {
        Err(storage_down())
    }

    async fn delete(&self, _atespace: &str, _actor: &str) -> ActorIndexResult<()> {
        Err(storage_down())
    }

    async fn all(&self) -> ActorIndexResult<Vec<ActorEntry>> {
        Err(storage_down())
    }

    async fn compare_exchange(
        &self,
        _expected: Option<&ActorEntry>,
        _replacement: Option<&ActorEntry>,
    ) -> ActorIndexResult<bool> {
        Err(storage_down())
    }
}
