//! `Provider` over Agent Substrate actors.

use std::collections::HashMap;
use std::num::NonZeroU16;
use std::sync::{Arc, Mutex as StdMutex, PoisonError};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Mutex, OwnedMutexGuard};
use tracing::{info, warn};
use trogon_sandbox_actor_agent::{
    BindLeaseRequest, MIN_AGENT_PROTOCOL_VERSION_ENV, MIN_SUPPORTED_AGENT_PROTOCOL_VERSION,
    min_protocol_version_from_env,
};
use trogon_sandbox_contract::{
    CancelIntent, GetSandboxDomainResponse, IsolationTier, LeaseId, LeaseRequest, Port, ProviderKind, ProviderLease,
    SandboxId, SandboxImage, SandboxPhase, SandboxSpec, TenantId, WorkerId, WorkspaceSource,
};
use trogon_sandbox_protocol::subjects::evidence_subject;
use trogon_sandbox_protocol::{
    AuthorityHost, AuthorityHostError, AuthorityResourceId, AuthorityTaskCompletion, AuthorityTaskScope, HeldLease,
    LifecycleEmitter, Provider, ProviderError, PurgeOutcome as AuthorityPurgeOutcome, lifecycle_subject, run_phase,
};
use trogon_std::env::ReadEnv;

use crate::actor_index::{ActorClaimId, ActorEntry, ActorIndex, ActorOwner, ActorUid, claim};
use crate::agent::{ActorAgent, ActorTarget};
use crate::ate_client::{AteError, Control};
use crate::config::{SIZE_CLASS_LABEL, SandboxClass, SubstrateConfig};
use crate::naming::{ActorName, Atespace};
use crate::proto;
use crate::template::{TemplateKey, TemplateRegistry};
use crate::workspace::{self, WORKSPACE_DIR};

mod active_lease;
mod constants;
mod purge_outcome;
mod retained_snapshot;
mod substrate_error;

pub use active_lease::ActiveLease;
pub use purge_outcome::PurgeOutcome;
pub use substrate_error::SubstrateError;

use constants::RESUME_POLL_INTERVAL;

/// Builds the per-lease lifecycle emitter.
///
/// The subject carries the lease id, so an emitter cannot be built once for the provider.
enum EmitterFactory {
    Nats(Box<async_nats::jetstream::Context>),
}

impl EmitterFactory {
    fn build(&self, lease_id: &LeaseId) -> Arc<dyn LifecycleEmitter> {
        match self {
            Self::Nats(js) => Arc::new(crate::nats_lifecycle::NatsLifecycleEmitter::new(
                (**js).clone(),
                lifecycle_subject(lease_id),
            )),
        }
    }
}

pub struct SubstrateProvider {
    worker_id: trogon_sandbox_contract::WorkerId,
    config: SubstrateConfig,
    control: Arc<dyn Control>,
    templates: TemplateRegistry,
    index: Arc<dyn ActorIndex>,
    active: Arc<Mutex<HashMap<String, ActiveLease>>>,
    custody: Mutex<HashMap<String, ActorCustody>>,
    completion: AuthorityTaskScope,
    actor_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    lifecycle: Option<EmitterFactory>,
    poll_interval: Duration,
    min_agent_protocol_version: u32,
}

/// An environment with no variables, so an agent built from it keeps its default settings
/// until the provider applies the ones it resolved once at construction.
struct EmptyEnv;

impl ReadEnv for EmptyEnv {
    fn var(&self, _key: &str) -> Result<String, std::env::VarError> {
        Err(std::env::VarError::NotPresent)
    }
}

#[derive(Clone)]
struct ActorCustody {
    entry: ActorEntry,
    resource: AuthorityResourceId,
    completion: AuthorityTaskCompletion,
    phase: PhaseCell,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ActorPhase {
    Preparing,
    Ready,
    Cleanup,
}

/// The phase of one held actor, shared between its custody record and its receipt.
///
/// A poisoned lock still holds a valid phase, so every access recovers the value instead
/// of failing.
#[derive(Clone)]
struct PhaseCell(Arc<StdMutex<ActorPhase>>);

impl PhaseCell {
    fn new(phase: ActorPhase) -> Self {
        Self(Arc::new(StdMutex::new(phase)))
    }

    fn get(&self) -> ActorPhase {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn set(&self, phase: ActorPhase) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = phase;
    }

    fn abandon_if_preparing(&self) {
        let mut phase = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if *phase == ActorPhase::Preparing {
            *phase = ActorPhase::Cleanup;
        }
    }
}

pub(crate) struct ProvisionReceipt(PhaseCell);

impl ProvisionReceipt {
    fn ready(&self) {
        self.0.set(ActorPhase::Ready);
    }
}

impl Drop for ProvisionReceipt {
    fn drop(&mut self) {
        self.0.abandon_if_preparing();
    }
}

impl SubstrateProvider {
    pub fn new(
        config: SubstrateConfig,
        control: Arc<dyn Control>,
        templates: TemplateRegistry,
        index: Arc<dyn ActorIndex>,
        env: &impl ReadEnv,
    ) -> Result<Self, SubstrateError> {
        Ok(Self {
            worker_id: WorkerId::new(format!("substrate-{}", ActorClaimId::generate().as_str()))?,
            config,
            control,
            templates,
            index,
            active: Arc::new(Mutex::new(HashMap::new())),
            custody: Mutex::new(HashMap::new()),
            completion: AuthorityTaskScope::default(),
            actor_locks: Arc::new(Mutex::new(HashMap::new())),
            lifecycle: None,
            poll_interval: RESUME_POLL_INTERVAL,
            min_agent_protocol_version: min_protocol_version_from_env(
                env,
                MIN_AGENT_PROTOCOL_VERSION_ENV,
                MIN_SUPPORTED_AGENT_PROTOCOL_VERSION,
            ),
        })
    }

    pub fn with_lifecycle_stream(mut self, js: async_nats::jetstream::Context) -> Self {
        self.lifecycle = Some(EmitterFactory::Nats(Box::new(js)));
        self
    }

    pub fn with_worker_id(mut self, worker_id: WorkerId) -> Self {
        self.worker_id = worker_id;
        self
    }

    pub fn worker_id(&self) -> &WorkerId {
        &self.worker_id
    }

    #[cfg(test)]
    fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    pub fn config(&self) -> &SubstrateConfig {
        &self.config
    }

    pub fn control(&self) -> &Arc<dyn Control> {
        &self.control
    }

    pub fn index(&self) -> &Arc<dyn ActorIndex> {
        &self.index
    }

    pub fn active(&self) -> &Arc<Mutex<HashMap<String, ActiveLease>>> {
        &self.active
    }

    /// Serializes lease and cancel for one actor.
    ///
    /// The broker clears a named lease the moment it publishes the cancel, so a re-lease of
    /// the same name can reach this worker while the old lease's suspend is still in flight.
    /// Without this, the resume sees a still-running actor and the late suspend then freezes
    /// it under the new lease.
    async fn lock_actor(&self, atespace: &Atespace, actor: &ActorName) -> OwnedMutexGuard<()> {
        let key = format!("{}/{}", atespace.as_str(), actor.as_str());
        let lock = self.actor_locks.lock().await.entry(key).or_default().clone();
        lock.lock_owned().await
    }

    async fn claimed_by_another_lease(&self, atespace: &Atespace, actor: &ActorName, excluding: &LeaseId) -> bool {
        self.active
            .lock()
            .await
            .iter()
            .any(|(id, lease)| id != excluding.as_str() && &lease.atespace == atespace && &lease.actor == actor)
    }

    pub async fn active_lease(&self, lease_id: &LeaseId) -> Option<ActiveLease> {
        self.active.lock().await.get(lease_id.as_str()).cloned()
    }

    pub async fn current_lease_count(&self) -> u32 {
        u32::try_from(self.active.lock().await.len()).unwrap_or(u32::MAX)
    }

    /// Restores a lease this worker fleet already owns, without touching the control plane.
    ///
    /// Used by recovery, which has already confirmed the actor is alive.
    pub(crate) async fn adopt(
        &self,
        lease_id: LeaseId,
        lease: ActiveLease,
        entry: ActorEntry,
        ready: bool,
    ) -> Result<ProvisionReceipt, SubstrateError> {
        let owner = entry
            .owner
            .as_ref()
            .ok_or_else(|| SubstrateError::Ownership("actor ownership is unbound".into()))?;
        let (resource, completion) = self.completion.track(
            lease_id.clone(),
            format!("{}/{}/{}", entry.atespace, entry.actor, owner.claim_id.as_str()),
        )?;
        let phase = PhaseCell::new(if ready {
            ActorPhase::Ready
        } else {
            ActorPhase::Preparing
        });
        let mut active = self.active.lock().await;
        let mut custody = self.custody.lock().await;
        custody.insert(
            lease_id.as_str().to_string(),
            ActorCustody {
                entry,
                resource,
                completion,
                phase: phase.clone(),
            },
        );
        active.insert(lease_id.as_str().to_string(), lease);
        Ok(ProvisionReceipt(phase))
    }

    /// The agent client for a lease, or `None` when this worker does not hold it.
    ///
    /// Every exec-surface handler starts here, and the `None` is what implements the
    /// broadcast routing rule: the broker sends `run_command` to every worker on the
    /// provider, and only the one holding the lease answers.
    fn actor_agent(&self, target: ActorTarget) -> ActorAgent {
        let agent = ActorAgent::new(self.config.router_endpoint.clone(), target, &EmptyEnv)
            .with_min_agent_protocol_version(self.min_agent_protocol_version);
        match &self.config.router_tls {
            Some(tls) => agent.with_router_tls(tls.clone()),
            None => agent,
        }
    }

    pub async fn agent_for(&self, lease_id: &LeaseId) -> Option<ActorAgent> {
        let lease = self.active_lease(lease_id).await?;
        let custody = self.custody.lock().await.get(lease_id.as_str()).cloned()?;
        let ready = custody.phase.get() == ActorPhase::Ready;
        if !ready || self.confirm_actor(&custody.entry).await.ok()?.is_none() {
            return None;
        }
        Some(self.actor_agent(ActorTarget::new(lease.atespace, lease.actor, self.config.agent_port)))
    }

    /// Resolves the single port an actor may expose.
    ///
    /// The router publishes one hostname per actor, so a port is either *the* exposed port
    /// or it is not exposed. There is no per-port endpoint to hand back.
    pub async fn sandbox_domain(&self, lease_id: &LeaseId, port: u16) -> GetSandboxDomainResponse {
        let Some(lease) = self.active_lease(lease_id).await else {
            return GetSandboxDomainResponse::PortNotExposed;
        };
        match lease.exposed_port {
            Some(exposed) if exposed.get() == port => GetSandboxDomainResponse::Available {
                endpoint: self
                    .config
                    .actor_endpoint(lease.actor.as_str(), lease.atespace.as_str()),
                url: None,
                expires_at: None,
                access: None,
            },
            _ => GetSandboxDomainResponse::PortNotExposed,
        }
    }

    async fn lease_inner(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, SubstrateError> {
        validate_spec(&req.spec)?;
        self.config.validate()?;
        crate::resources::validate_image_digest(&image_reference(req.spec.image())?)?;

        let atespace = Atespace::for_tenant(&req.tenant)?;
        let class = self.sandbox_class_for(req)?;
        let memory_mib = req.spec.memory_mib().get();
        let selector = self.config.size_selector(memory_mib).ok_or_else(|| {
            SubstrateError::InvalidSpec(format!(
                "memory_mib {memory_mib} exceeds the largest configured substrate size class"
            ))
        })?;

        let actor = match &req.name {
            Some(name) => ActorName::for_named_lease(&atespace, name.as_str()),
            None => ActorName::for_lease(lease_id)?,
        };

        self.ensure_atespace(&atespace).await?;

        let _actor_guard = self.lock_actor(&atespace, &actor).await;

        // The template's own worker_selector must ask for the same size class as the actor's,
        // since a worker carries only one `ate.dev/size` label and the two selectors are
        // AND-combined: reusing the label already computed for the actor keeps them in lock
        // step instead of risking two independent lookups drifting apart.
        let size_class_label = selector
            .match_labels
            .get(SIZE_CLASS_LABEL)
            .ok_or(SubstrateError::UnlabelledSizeSelector)?;

        let resources = self.config.resources_for(memory_mib)?;
        if class == SandboxClass::MicroVm && resources.memory_mib() < 128 {
            return Err(SubstrateError::InvalidSpec(
                "microvm actors require at least 128 MiB".into(),
            ));
        }
        let key = TemplateKey::new(
            &image_reference(req.spec.image())?,
            class,
            &self.config,
            size_class_label,
            resources,
        );

        let emitter = self.lifecycle.as_ref().map(|f| f.build(lease_id));
        let emitter = emitter.as_deref();
        let template_ref = run_phase(emitter, SandboxPhase::PullingImage, None, || async {
            Ok::<_, SubstrateError>(
                self.templates
                    .ensure_ready(
                        self.control.as_ref(),
                        &self.config,
                        atespace.as_str(),
                        &key,
                        self.config.template_build_timeout,
                    )
                    .await?,
            )
        })
        .await?;

        let actor_ref = proto::ObjectRef {
            atespace: atespace.as_str().to_string(),
            name: actor.as_str().to_string(),
        };

        let prior = self
            .index
            .get(atespace.as_str(), actor.as_str())
            .await
            .map_err(|error| SubstrateError::Ownership(error.to_string()))?;
        if let Some(entry) = &prior {
            if entry.lease_id == *lease_id && !entry.retained {
                let held = self.custody.lock().await.get(lease_id.as_str()).cloned();
                if held
                    .as_ref()
                    .is_some_and(|held| held.entry == *entry && held.phase.get() == ActorPhase::Ready)
                {
                    self.confirm_actor(entry)
                        .await?
                        .ok_or_else(|| SubstrateError::Ownership("held actor is absent".into()))?;
                    return self.grant(lease_id, format!("{}/{}", atespace.as_str(), actor.as_str()));
                }
            }
            if !entry.retained
                || !entry.named
                || req.name.is_none()
                || entry
                    .owner
                    .as_ref()
                    .and_then(|owner| owner.actor_uid.as_ref())
                    .is_none()
            {
                return Err(SubstrateError::Ownership(
                    "actor is held by another or unconfirmed assignment".into(),
                ));
            }
            let existing = self.confirm_actor(entry).await?.ok_or_else(|| {
                SubstrateError::Ownership(
                    "retained actor is absent; reconcile its claim before reusing its name".into(),
                )
            })?;
            if actor_state(&existing) != proto::ActorState::Suspended {
                return Err(SubstrateError::Ownership(
                    "retained actor is not positively suspended".into(),
                ));
            }
            self.verify_template(&actor_ref, &template_ref, &existing)?;
        }
        let mut ownership = claim(&atespace, &actor, lease_id);
        ownership.owner = Some(ActorOwner {
            worker_id: self.worker_id.clone(),
            claim_id: ActorClaimId::generate(),
            actor_uid: prior
                .as_ref()
                .and_then(|entry| entry.owner.as_ref())
                .and_then(|owner| owner.actor_uid.clone()),
        });
        self.exchange_ownership(prior.as_ref(), Some(&ownership)).await?;

        let exposed_port = req.spec.exposed_ports().first().copied();
        let lease = ActiveLease {
            atespace: atespace.clone(),
            actor: actor.clone(),
            named: req.name.is_some(),
            persistent: req.persistent.is_persistent(),
            exposed_port,
        };
        let sandbox_id = lease.sandbox_id();
        let mut provision = if prior.is_some() {
            Some(
                self.adopt(lease_id.clone(), lease.clone(), ownership.clone(), false)
                    .await?,
            )
        } else {
            None
        };

        let existed = run_phase(emitter, SandboxPhase::Allocate, None, || async {
            if prior.is_some() {
                self.confirm_actor(&ownership)
                    .await?
                    .ok_or_else(|| SubstrateError::Ownership("retained actor disappeared".into()))?;
                let restored = self
                    .ensure_actor(
                        &actor_ref,
                        &template_ref,
                        selector.clone(),
                        ownership.owner.as_ref().and_then(|owner| owner.actor_uid.as_ref()),
                    )
                    .await?;
                Ok(restored.1)
            } else {
                let created = self
                    .ensure_actor(&actor_ref, &template_ref, selector.clone(), None)
                    .await?;
                let uid = actor_uid(&created.0)?;
                let mut bound = ownership.clone();
                bound
                    .owner
                    .as_mut()
                    .ok_or_else(|| SubstrateError::Ownership("actor ownership is unbound".into()))?
                    .actor_uid = Some(uid);
                provision = Some(
                    self.adopt(lease_id.clone(), lease.clone(), bound.clone(), false)
                        .await?,
                );
                self.exchange_ownership(Some(&ownership), Some(&bound)).await?;
                ownership = bound;
                Ok::<_, SubstrateError>(created.1)
            }
        })
        .await?;

        // A pre-existing actor has a golden or self-captured snapshot worth restoring; a
        // fresh one has nothing to restore and must boot. The phase label follows that split
        // so a timeline shows which path a lease actually took, even though ResumeActor
        // itself no longer takes a boot/warm flag: the actor's own state decides that.
        let phase = if existed {
            SandboxPhase::RestoreSnapshot
        } else {
            SandboxPhase::BootRuntime
        };

        run_phase(emitter, phase, None, || async {
            self.confirm_actor(&ownership)
                .await?
                .ok_or_else(|| SubstrateError::Ownership("allocated actor disappeared".into()))?;
            self.resume(&actor_ref).await
        })
        .await?;

        let agent = self.actor_agent(ActorTarget::new(
            atespace.clone(),
            actor.clone(),
            self.config.agent_port,
        ));
        let working_dir = req
            .spec
            .working_dir()
            .map_or_else(|| WORKSPACE_DIR.to_string(), str::to_string);

        run_phase(emitter, SandboxPhase::PrepareWorkspace, None, || async {
            self.confirm_actor(&ownership)
                .await?
                .ok_or_else(|| SubstrateError::Ownership("allocated actor disappeared".into()))?;
            self.prepare(&agent, req, lease_id, exposed_port, &working_dir).await
        })
        .await?;

        provision
            .as_ref()
            .ok_or_else(|| SubstrateError::Ownership("allocation left no provisioning receipt".into()))?
            .ready();

        info!(
            lease_id = %lease_id,
            actor = %actor,
            atespace = %atespace,
            existed,
            "substrate lease ready"
        );

        self.grant(lease_id, sandbox_id)
    }

    fn grant(&self, lease_id: &LeaseId, sandbox_id: String) -> Result<ProviderLease, SubstrateError> {
        Ok(ProviderLease {
            epoch: None,
            sandbox_id: SandboxId::new(sandbox_id)?,
            evidence_subject: evidence_subject(lease_id),
            worker_id: Some(self.worker_id.clone()),
            attempt_id: None,
        })
    }

    fn sandbox_class_for(&self, req: &LeaseRequest) -> Result<SandboxClass, SubstrateError> {
        match req.isolation_tier {
            None => Ok(self.config.sandbox_class),
            Some(tier) => SandboxClass::for_tier(tier).ok_or_else(|| {
                SubstrateError::InvalidSpec(format!(
                    "isolation tier {tier:?} is not available on the substrate provider"
                ))
            }),
        }
    }

    async fn ensure_atespace(&self, atespace: &Atespace) -> Result<(), SubstrateError> {
        match self.control.create_atespace(atespace.as_str()).await {
            Ok(_) | Err(AteError::AlreadyExists { .. }) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Creates the actor, or adopts the one already there. Returns whether it pre-existed.
    async fn ensure_actor(
        &self,
        actor_ref: &proto::ObjectRef,
        template_ref: &proto::ObjectRef,
        selector: proto::Selector,
        expected_uid: Option<&ActorUid>,
    ) -> Result<(proto::Actor, bool), SubstrateError> {
        if let Some(expected_uid) = expected_uid {
            let existing = self.control.get_actor(actor_ref.clone()).await?;
            if actor_uid(&existing)? != *expected_uid {
                return Err(SubstrateError::Ownership("actor resource UID changed".into()));
            }
            self.verify_template(actor_ref, template_ref, &existing)?;
            return Ok((existing, true));
        }

        let actor = proto::Actor {
            metadata: Some(proto::ResourceMetadata {
                atespace: actor_ref.atespace.clone(),
                name: actor_ref.name.clone(),
                ..Default::default()
            }),
            actor_template: Some(template_ref.clone()),
            worker_selector: Some(selector),
            ..Default::default()
        };

        match self.control.create_actor(actor).await {
            Ok(created) => {
                actor_uid(&created)?;
                if created
                    .metadata
                    .as_ref()
                    .is_none_or(|metadata| metadata.atespace != actor_ref.atespace || metadata.name != actor_ref.name)
                {
                    return Err(SubstrateError::Ownership(
                        "allocation reply names a different actor".into(),
                    ));
                }
                Ok((created, false))
            }
            Err(AteError::AlreadyExists { .. }) => Err(SubstrateError::Ownership(
                "existing actor has no confirmed allocation receipt".into(),
            )),
            Err(error) => Err(error.into()),
        }
    }

    /// A named lease whose image changed would otherwise silently keep running the old one.
    ///
    /// Refusing is the honest outcome: the actor's state belongs to the old image, and
    /// recreating it under the new template would discard exactly the state the tenant
    /// asked for by name.
    fn verify_template(
        &self,
        actor_ref: &proto::ObjectRef,
        wanted: &proto::ObjectRef,
        existing: &proto::Actor,
    ) -> Result<(), SubstrateError> {
        let found = existing
            .actor_template
            .as_ref()
            .map(|v| v.name.clone())
            .unwrap_or_default();
        if found == wanted.name
            && existing
                .actor_template
                .as_ref()
                .is_some_and(|template| template.atespace == wanted.atespace)
        {
            return Ok(());
        }
        Err(SubstrateError::TemplateMismatch {
            actor: actor_ref.name.clone(),
            wanted: wanted.name.clone(),
            found,
        })
    }

    /// Resumes and waits for `ACTOR_STATE_RUNNING`.
    ///
    /// `resumed: false` from `ResumeActor` means the actor was already running, which is the
    /// correct outcome for a retried dispatch rather than a failure. `ResumeActor` carries no
    /// boot/warm flag of its own: the actor's own state and template settings decide whether
    /// this is a cold start or a snapshot restore.
    async fn resume(&self, actor_ref: &proto::ObjectRef) -> Result<(), SubstrateError> {
        let actor = self.control.resume_actor(actor_ref.clone()).await?;
        if actor_state(&actor) == proto::ActorState::Running {
            return Ok(());
        }

        let started = tokio::time::Instant::now();
        loop {
            let actor = self.control.get_actor(actor_ref.clone()).await?;
            match actor_state(&actor) {
                proto::ActorState::Running => return Ok(()),
                // A crash during resume is terminal: retrying the same resume reproduces it,
                // and the lease should fail now rather than after the full timeout.
                proto::ActorState::Crashed => {
                    return Err(SubstrateError::Ate(AteError::Other {
                        message: format!("actor {} crashed during resume", actor_ref.name),
                    }));
                }
                status => {
                    if started.elapsed() >= self.config.resume_timeout {
                        return Err(SubstrateError::ResumeTimeout {
                            actor: actor_ref.name.clone(),
                            status: format!("{status:?}"),
                            seconds: self.config.resume_timeout.as_secs(),
                        });
                    }
                }
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    async fn prepare(
        &self,
        agent: &ActorAgent,
        req: &LeaseRequest,
        lease_id: &LeaseId,
        exposed_port: Option<Port>,
        working_dir: &str,
    ) -> Result<(), SubstrateError> {
        agent.wait_ready(self.config.resume_timeout).await?;

        agent
            .bind(&BindLeaseRequest {
                entropy_seed: None,
                lease_id: lease_id.clone(),
                env: req.spec.env().clone(),
                working_dir: Some(working_dir.to_string()),
                exposed_port: exposed_port.and_then(|port| NonZeroU16::new(port.get())),
            })
            .await?;

        workspace::materialize(agent, lease_id, req.spec.workspace(), working_dir).await?;
        Ok(())
    }

    async fn cancel_inner(&self, lease_id: &LeaseId, intent: CancelIntent) -> Result<(), SubstrateError> {
        self.cancel_expected(lease_id, intent, None).await.map(|_| ())
    }

    async fn cancel_expected(
        &self,
        lease_id: &LeaseId,
        intent: CancelIntent,
        expected: Option<&AuthorityResourceId>,
    ) -> Result<bool, SubstrateError> {
        let Some(lease) = self.active.lock().await.get(lease_id.as_str()).cloned() else {
            // Not ours. The broker broadcasts cancels, so this is the normal case for every
            // worker but one, and must not be an error.
            return Ok(false);
        };

        let _actor_guard = self.lock_actor(&lease.atespace, &lease.actor).await;
        let custody = self
            .custody
            .lock()
            .await
            .get(lease_id.as_str())
            .cloned()
            .ok_or_else(|| SubstrateError::Ownership("held actor has no allocation receipt".into()))?;
        if expected.is_some_and(|expected| *expected != custody.resource) {
            return Ok(false);
        }
        if self
            .claimed_by_another_lease(&lease.atespace, &lease.actor, lease_id)
            .await
        {
            self.active.lock().await.remove(lease_id.as_str());
            self.custody.lock().await.remove(lease_id.as_str());
            info!(actor = %lease.actor, "actor already handed to a newer lease; leaving it running");
            return Ok(false);
        }

        let ownership = &custody.entry;
        if ownership.lease_id != *lease_id
            || ownership
                .owner
                .as_ref()
                .is_none_or(|owner| owner.worker_id != self.worker_id)
        {
            return Err(SubstrateError::Ownership(
                "actor claim belongs to another assignment".into(),
            ));
        }

        // A named, persistent lease's actor *is* the tenant's saved state, so cancel stops
        // at suspended and leaves it. Everything else is disposable.
        let mut retain = lease.named && lease.persistent && intent == CancelIntent::Release;

        match self.settle_owned(ownership).await {
            Ok(Settled::Gone) => {
                self.exchange_ownership(Some(ownership), None).await?;
                self.active.lock().await.remove(lease_id.as_str());
                self.custody.lock().await.remove(lease_id.as_str());
                custody.completion.confirm();
                return Ok(true);
            }
            Ok(Settled::Suspended) => {}
            // A crash may leave durable snapshot history. Ordinary release cannot discard
            // that history for a persistent name; explicit purge still reclaims the actor.
            Ok(Settled::Crashed) => {
                if retain {
                    return Err(SubstrateError::Ownership(
                        "crashed named actor may retain durable snapshot history; explicit purge is required".into(),
                    ));
                }
                warn!(actor = %lease.actor, "actor crashed; deleting instead of retaining it");
                retain = false;
            }
            Err(e) => {
                warn!(error = %e, actor = %lease.actor, "failed to suspend actor during cancel; retaining its cleanup receipt");
                return Err(e);
            }
        }

        if retain {
            self.confirm_retained_snapshot(ownership).await?;
            let mut retained = ownership.clone();
            retained.retained = true;
            self.exchange_ownership(Some(ownership), Some(&retained)).await?;
            self.active.lock().await.remove(lease_id.as_str());
            self.custody.lock().await.remove(lease_id.as_str());
            info!(actor = %lease.actor, "retaining suspended actor for a named lease");
            return Ok(true);
        }

        self.delete_owned(ownership).await?;
        self.exchange_ownership(Some(ownership), None).await?;
        self.active.lock().await.remove(lease_id.as_str());
        self.custody.lock().await.remove(lease_id.as_str());
        custody.completion.confirm();
        Ok(true)
    }

    async fn confirm_actor(&self, entry: &ActorEntry) -> Result<Option<proto::Actor>, SubstrateError> {
        let stored = self
            .index
            .get(&entry.atespace, &entry.actor)
            .await
            .map_err(|error| SubstrateError::Ownership(error.to_string()))?;
        if stored.as_ref() != Some(entry) {
            let mut pending = entry.clone();
            let owner = pending
                .owner
                .as_mut()
                .ok_or_else(|| SubstrateError::Ownership("actor ownership is unbound".into()))?;
            owner.actor_uid = None;
            let local = self
                .custody
                .lock()
                .await
                .get(entry.lease_id.as_str())
                .is_some_and(|held| held.entry == *entry);
            if owner.worker_id != self.worker_id || stored.as_ref() != Some(&pending) || !local {
                return Err(SubstrateError::Ownership(
                    "actor claim belongs to another or unconfirmed assignment".into(),
                ));
            }
            let actor = self.read_owned_actor(entry).await?;
            self.exchange_ownership(Some(&pending), Some(entry)).await?;
            return Ok(actor);
        }
        self.exchange_ownership(Some(entry), Some(entry)).await?;
        self.read_owned_actor(entry).await
    }

    async fn read_owned_actor(&self, entry: &ActorEntry) -> Result<Option<proto::Actor>, SubstrateError> {
        let uid = entry
            .owner
            .as_ref()
            .and_then(|owner| owner.actor_uid.as_ref())
            .ok_or_else(|| SubstrateError::Ownership("actor resource UID is unconfirmed".into()))?;
        let actor_ref = proto::ObjectRef {
            atespace: entry.atespace.clone(),
            name: entry.actor.clone(),
        };
        match self.control.get_actor(actor_ref).await {
            Ok(actor) => {
                let metadata = actor
                    .metadata
                    .as_ref()
                    .ok_or_else(|| SubstrateError::Ownership("actor metadata is missing".into()))?;
                if metadata.atespace != entry.atespace || metadata.name != entry.actor || actor_uid(&actor)? != *uid {
                    return Err(SubstrateError::Ownership(
                        "actor resource UID or address changed".into(),
                    ));
                }
                Ok(Some(actor))
            }
            Err(AteError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) async fn delete_owned(&self, entry: &ActorEntry) -> Result<(), SubstrateError> {
        let Some(observed) = self.confirm_actor(entry).await? else {
            return Ok(());
        };
        let revision = crate::ate_client::ResourceRevision::from_metadata(observed.metadata.as_ref())?;
        match self.control.delete_actor(revision).await {
            Ok(_) | Err(AteError::NotFound { .. }) => {}
            Err(error) => return Err(error.into()),
        }
        if self.confirm_actor(entry).await?.is_some() {
            return Err(SubstrateError::Ownership("actor deletion remains unconfirmed".into()));
        }
        Ok(())
    }

    async fn settle_owned(&self, entry: &ActorEntry) -> Result<Settled, SubstrateError> {
        let started = tokio::time::Instant::now();
        let actor_ref = proto::ObjectRef {
            atespace: entry.atespace.clone(),
            name: entry.actor.clone(),
        };
        loop {
            let Some(actor) = self.confirm_actor(entry).await? else {
                return Ok(Settled::Gone);
            };
            let state = actor_state(&actor);
            match state {
                proto::ActorState::Suspended => return Ok(Settled::Suspended),
                proto::ActorState::Crashed => return Ok(Settled::Crashed),
                proto::ActorState::Running | proto::ActorState::Resuming => {
                    self.suspend_ignoring_races(&actor_ref).await?;
                }
                _ => {}
            }
            if started.elapsed() >= self.config.resume_timeout {
                return Err(SubstrateError::ResumeTimeout {
                    actor: entry.actor.clone(),
                    status: format!("{state:?}"),
                    seconds: self.config.resume_timeout.as_secs(),
                });
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    async fn confirm_retained_snapshot(&self, entry: &ActorEntry) -> Result<(), SubstrateError> {
        let started = tokio::time::Instant::now();
        loop {
            let observed = self.confirm_actor(entry).await?.ok_or_else(|| {
                SubstrateError::Ownership("actor disappeared before retained state was confirmed".into())
            })?;
            if let Some(evidence) = retained_snapshot::durable_memory_snapshot(&observed)? {
                info!(actor = %entry.actor, actor_uid = actor_uid(&observed)?.as_str(), snapshot = %evidence.uuid, generation = evidence.generation, "observed durable memory snapshot for retained actor");
                return Ok(());
            }
            if started.elapsed() >= self.config.resume_timeout {
                return Err(SubstrateError::ResumeTimeout {
                    actor: entry.actor.clone(),
                    status: "pending durable snapshot upload".into(),
                    seconds: self.config.resume_timeout.as_secs(),
                });
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }

    async fn exchange_ownership(
        &self,
        expected: Option<&crate::actor_index::ActorEntry>,
        replacement: Option<&crate::actor_index::ActorEntry>,
    ) -> Result<(), SubstrateError> {
        if expected.is_none() && replacement.is_none() {
            return Ok(());
        }
        match self.index.compare_exchange(expected, replacement).await {
            Ok(true) => Ok(()),
            Ok(false) => Err(SubstrateError::Ownership("actor claim changed during cleanup".into())),
            Err(error) => Err(SubstrateError::Ownership(error.to_string())),
        }
    }

    pub(crate) async fn reap_owned(&self, entry: &ActorEntry) -> Result<bool, SubstrateError> {
        if entry.retained
            || entry
                .owner
                .as_ref()
                .is_none_or(|owner| owner.worker_id != self.worker_id || owner.actor_uid.is_none())
        {
            return Ok(false);
        }
        let atespace = Atespace::for_tenant(&TenantId::new(entry.atespace.clone())?)?;
        let actor = entry
            .actor_name()
            .ok_or_else(|| SubstrateError::Ownership("unrecognized actor identity".into()))?;
        let _guard = self.lock_actor(&atespace, &actor).await;
        if self
            .active
            .lock()
            .await
            .values()
            .any(|lease| lease.atespace == atespace && lease.actor == actor)
        {
            return Ok(false);
        }
        self.settle_owned(entry).await?;
        self.delete_owned(entry).await?;
        self.exchange_ownership(Some(entry), None).await?;
        Ok(true)
    }

    async fn suspend_ignoring_races(&self, actor_ref: &proto::ObjectRef) -> Result<(), SubstrateError> {
        match self.control.suspend_actor(actor_ref.clone()).await {
            Ok(_) => Ok(()),
            // The status we read is a snapshot; by the time we act on it the actor may have
            // moved on by itself. Both of these mean "keep polling", not "give up".
            Err(AteError::FailedPrecondition { .. }) | Err(AteError::NotFound { .. }) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Deletes the actor backing a named lease by name alone, with no lease in hand.
    ///
    /// Reached straight from the tenant's `delete_named_sandbox` request, after a stop_lease
    /// has already left the actor suspended with nothing claiming it. The broker purges any
    /// lease still holding the name before it gets here, but nothing at this layer enforces
    /// that ordering, so a claim found here is evicted rather than trusted.
    pub async fn purge_named(&self, tenant: &TenantId, name: &str) -> Result<PurgeOutcome, SubstrateError> {
        let atespace = Atespace::for_tenant(tenant)?;
        let actor = ActorName::for_named_lease(&atespace, name);
        let _actor_guard = self.lock_actor(&atespace, &actor).await;

        let actor_ref = proto::ObjectRef {
            atespace: atespace.as_str().to_string(),
            name: actor.as_str().to_string(),
        };
        let Some(mut entry) = self
            .index
            .get(atespace.as_str(), actor.as_str())
            .await
            .map_err(|error| SubstrateError::Ownership(error.to_string()))?
        else {
            return match self.control.get_actor(actor_ref).await {
                Err(AteError::NotFound { .. }) => Ok(PurgeOutcome::NotFound),
                Err(error) => Err(error.into()),
                Ok(_) => Err(SubstrateError::Ownership(
                    "actor has no confirmed fleet ownership".into(),
                )),
            };
        };
        if !entry.retained
            && entry
                .owner
                .as_ref()
                .is_none_or(|owner| owner.worker_id != self.worker_id)
        {
            return Err(SubstrateError::Ownership(
                "actor belongs to another active worker".into(),
            ));
        }
        if entry.retained {
            if let Some(parked) = self.confirm_actor(&entry).await?
                && actor_state(&parked) != proto::ActorState::Suspended
            {
                return Err(SubstrateError::Ownership(
                    "retained actor is not positively suspended".into(),
                ));
            }
            let mut cleanup = entry.clone();
            cleanup.retained = false;
            let owner = cleanup
                .owner
                .as_mut()
                .ok_or_else(|| SubstrateError::Ownership("actor ownership is unbound".into()))?;
            owner.worker_id = self.worker_id.clone();
            owner.claim_id = ActorClaimId::generate();
            self.exchange_ownership(Some(&entry), Some(&cleanup)).await?;
            self.adopt(
                cleanup.lease_id.clone(),
                ActiveLease {
                    atespace: atespace.clone(),
                    actor: actor.clone(),
                    named: true,
                    persistent: true,
                    exposed_port: None,
                },
                cleanup.clone(),
                false,
            )
            .await?;
            entry = cleanup;
        }
        let outcome = if self.settle_owned(&entry).await? == Settled::Gone {
            PurgeOutcome::NotFound
        } else {
            self.delete_owned(&entry).await?;
            PurgeOutcome::Purged
        };
        self.exchange_ownership(Some(&entry), None).await?;
        self.active
            .lock()
            .await
            .retain(|_, lease| !(lease.atespace == atespace && lease.actor == actor));
        self.custody.lock().await.retain(|_, held| {
            if held.entry == entry {
                held.completion.confirm();
                false
            } else {
                true
            }
        });
        Ok(outcome)
    }
}

#[async_trait]
impl AuthorityHost for SubstrateProvider {
    async fn held_leases(&self) -> Result<Vec<HeldLease>, AuthorityHostError> {
        Ok(self
            .custody
            .lock()
            .await
            .values()
            .filter(|held| held.phase.get() != ActorPhase::Preparing)
            .map(|held| HeldLease::Provider {
                lease_id: held.entry.lease_id.clone(),
                provider: ProviderKind::Substrate,
                resource_id: held.resource.clone(),
            })
            .collect())
    }

    async fn purge(&self, lease: &HeldLease) -> Result<AuthorityPurgeOutcome, AuthorityHostError> {
        let HeldLease::Provider {
            lease_id,
            provider: ProviderKind::Substrate,
            resource_id,
        } = lease
        else {
            return Ok(AuthorityPurgeOutcome::Stale);
        };
        if self.completion.is_completed(lease_id, resource_id) {
            return Ok(AuthorityPurgeOutcome::Settled);
        }
        match self
            .cancel_expected(lease_id, CancelIntent::Purge, Some(resource_id))
            .await
        {
            Ok(true) => Ok(AuthorityPurgeOutcome::Purged),
            Ok(false) => Ok(AuthorityPurgeOutcome::Stale),
            Err(error) => Err(AuthorityHostError::Failed(Box::new(error))),
        }
    }
}

fn actor_uid(actor: &proto::Actor) -> Result<ActorUid, SubstrateError> {
    actor
        .metadata
        .as_ref()
        .and_then(|metadata| ActorUid::try_from(metadata.uid.clone()).ok())
        .ok_or_else(|| SubstrateError::Ownership("actor resource UID is unconfirmed".into()))
}

/// Outcome of driving an actor toward a deletable state.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum Settled {
    Suspended,
    Crashed,
    Gone,
}

#[async_trait]
impl Provider for SubstrateProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Substrate
    }

    async fn lease(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        self.lease_inner(req, lease_id).await.map_err(ProviderError::backend)
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        self.cancel_inner(lease_id, CancelIntent::Release)
            .await
            .map_err(ProviderError::backend)
    }

    async fn purge(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        self.cancel_inner(lease_id, CancelIntent::Purge)
            .await
            .map_err(ProviderError::backend)
    }
}

/// An actor's lifecycle state, defaulting to `ACTOR_STATE_UNSPECIFIED` when the server has
/// not populated `status` at all (never observed in practice, but `status` is `optional`).
fn actor_state(actor: &proto::Actor) -> proto::ActorState {
    actor.status.as_ref().map(|status| status.state()).unwrap_or_default()
}

/// The tenant image a template is built from.
fn image_reference(image: &SandboxImage) -> Result<String, SubstrateError> {
    // Every variant but a registry image means "start from an artifact we produced", and this
    // provider cannot: an actor is created from a substrate `SnapshotRef` the control plane
    // owns, never from an OCI artifact.
    let kind = match image {
        SandboxImage::Registry { reference, .. } => return Ok(reference.allowlist_string()),
        SandboxImage::Snapshot { .. } => "a snapshot artifact",
        SandboxImage::SnapshotTag { .. } => "a snapshot tag",
        SandboxImage::HyperlightModule { .. } => "a hyperlight module",
        SandboxImage::NativeSnapshot { .. } => "a native provider snapshot",
    };
    Err(SubstrateError::InvalidSpec(format!(
        "the substrate provider starts actors from registry images only, got {kind}"
    )))
}

/// The worker's own copy of the broker's pre-dispatch checks.
///
/// Duplicated on purpose. The broker rejects these as `InvalidSpec`, which is what a tenant
/// should see; by the time a spec reaches here a rejection can only surface as a backend
/// error. That makes this the wrong place to *report* the problem but the right place to
/// stop it, because a worker that trusts its input has no defence against a broker bug.
fn validate_spec(spec: &SandboxSpec) -> Result<(), SubstrateError> {
    if spec.vcpu_millis().is_some() {
        return Err(SubstrateError::InvalidSpec(
            "vcpu_millis is unsupported by the substrate worker; actor CPU capacity is configured by the operator"
                .into(),
        ));
    }
    if !spec.env().is_empty() {
        return Err(SubstrateError::InvalidSpec(
            "spec.env is not supported on the substrate provider".to_string(),
        ));
    }
    if !spec.mounts().is_empty() {
        return Err(SubstrateError::InvalidSpec(
            "remote mounts are not supported on the substrate provider: the actor sandbox has no FUSE".to_string(),
        ));
    }
    if !spec.drives().is_empty() {
        return Err(SubstrateError::InvalidSpec(
            "persistent drives are not supported on the substrate provider: actor volumes do not outlive the actor"
                .to_string(),
        ));
    }
    if spec.privileged().is_granted() {
        return Err(SubstrateError::InvalidSpec(
            "privileged: true is not supported on the substrate provider: it has no capability plumbing".to_string(),
        ));
    }
    if spec.user().is_some() {
        return Err(SubstrateError::InvalidSpec(
            "user overrides are not supported on the substrate provider: the container user is frozen into the actor template".to_string(),
        ));
    }
    if spec.exposed_ports().len() > 1 {
        return Err(SubstrateError::InvalidSpec(format!(
            "the substrate provider exposes at most one port per sandbox, {} were requested",
            spec.exposed_ports().len()
        )));
    }
    if let WorkspaceSource::HostMount { .. } = spec.workspace() {
        return Err(SubstrateError::InvalidSpec(
            "host_mount workspaces are not supported on the substrate provider".to_string(),
        ));
    }
    Ok(())
}

/// Whether a tier is one this provider advertises. Used by the worker's capability replies.
pub fn supports_tier(tier: IsolationTier) -> bool {
    SandboxClass::for_tier(tier).is_some()
}

#[cfg(test)]
pub(crate) mod tests_support;

#[cfg(test)]
mod tests;
