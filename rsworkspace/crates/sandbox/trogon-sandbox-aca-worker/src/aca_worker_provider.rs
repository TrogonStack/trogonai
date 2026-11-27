use std::collections::HashMap;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex as StdMutex, OnceLock, PoisonError, Weak};

use async_nats::jetstream;
use async_trait::async_trait;
use tokio::sync::Mutex;
use tracing::{info, warn};
use trogon_sandbox_contract::{
    CredentialProvider, CredentialScope, LeaseId, LeaseRequest, LifecycleEvent, ProviderKind, ProviderLease, SandboxId,
    SandboxPhase, ShortLivedToken, TenantId, WorkerId,
};
use trogon_sandbox_protocol::subjects::evidence_subject;
use trogon_sandbox_protocol::{
    AuthorityResourceId, AuthorityTaskScope, LifecycleEmitter, Provider, ProviderError, lifecycle_subject,
    reject_unsupported_privileged, run_phase,
};

use crate::active_aca_lease::ActiveAcaLease;
use crate::constants::{
    CLEANUP_CREDENTIAL_TTL_SECONDS, MAX_LEASE_LIFETIME_SECONDS, SESSION_READY_ATTEMPTS, SESSION_READY_POLL_INTERVAL,
};
use crate::{
    AcaError, AcaSessionApi, AzureVolumeBackends, CredentialIssuer, NatsLifecycleEmitter, PROFILE, ProviderConfig,
    ReqwestAcaClient, ServicePrincipal, SessionDeleteOutcome, SessionIdentifier, VolumeBackends, VolumeSupport,
    WorkerBindingError, credential_ttl_seconds, fetch_dynamic_sessions_token, image_env_vars, materialize_workspace,
    resolve_registry_image, upload_directory, wait_session_ready,
};

type EmitterFactory = Box<dyn Fn(&LeaseId) -> Arc<dyn LifecycleEmitter> + Send + Sync>;

/// Fulfills leases with Azure Container Apps Dynamic Sessions: one session per lease, seeded with
/// the lease workspace and deleted when the lease ends.
pub struct AcaWorkerProvider {
    config: ProviderConfig,
    max_lease_lifetime: NonZeroU32,
    api: Arc<dyn AcaSessionApi>,
    issuer: Arc<dyn CredentialIssuer>,
    volumes: VolumeSupport,
    http: reqwest::Client,
    auth_bypass: Option<ShortLivedToken>,
    emitters: EmitterFactory,
    pub(crate) active: Mutex<HashMap<LeaseId, ActiveAcaLease>>,
    gates: StdMutex<HashMap<LeaseId, Weak<Mutex<()>>>>,
    worker_id: OnceLock<WorkerId>,
    pub(crate) scope: AuthorityTaskScope,
}

impl AcaWorkerProvider {
    fn build(config: ProviderConfig, issuer: Arc<dyn CredentialIssuer>, emitters: EmitterFactory) -> Self {
        let volumes = VolumeSupport {
            issuer: Arc::clone(&issuer),
            backends: Arc::new(AzureVolumeBackends),
            storage_account: config.volume_storage_account.clone(),
            credential_id: config.volume_credential_id.clone(),
        };
        Self {
            api: Arc::new(ReqwestAcaClient::new(config.management_endpoint.clone())),
            config,
            max_lease_lifetime: MAX_LEASE_LIFETIME_SECONDS,
            issuer,
            volumes,
            http: reqwest::Client::new(),
            auth_bypass: None,
            emitters,
            active: Mutex::default(),
            gates: StdMutex::default(),
            worker_id: OnceLock::new(),
            scope: AuthorityTaskScope::default(),
        }
    }

    /// A provider that publishes each lease's lifecycle events to its JetStream subject.
    pub fn new(js: jetstream::Context, config: ProviderConfig, issuer: Arc<dyn CredentialIssuer>) -> Self {
        Self::build(
            config,
            issuer,
            Box::new(move |lease| Arc::new(NatsLifecycleEmitter::new(js.clone(), lifecycle_subject(lease)))),
        )
    }

    /// A provider that hands every lease's lifecycle events to `emitter`.
    pub fn with_emitter(
        emitter: Arc<dyn LifecycleEmitter>,
        config: ProviderConfig,
        issuer: Arc<dyn CredentialIssuer>,
    ) -> Self {
        Self::build(config, issuer, Box::new(move |_| Arc::clone(&emitter)))
    }

    /// Talks to the data plane through `api` instead of HTTP.
    pub fn with_client(mut self, api: Arc<dyn AcaSessionApi>) -> Self {
        self.api = api;
        self
    }

    /// Syncs volumes through `backends` instead of Azure Blob Storage.
    pub fn with_volume_backends(mut self, backends: Arc<dyn VolumeBackends>) -> Self {
        self.volumes.backends = backends;
        self
    }

    /// Caps how long credentials issued for a lease stay valid.
    pub fn with_max_lease_lifetime(mut self, max_lease_lifetime: NonZeroU32) -> Self {
        self.max_lease_lifetime = max_lease_lifetime;
        self
    }

    /// Uses `token` as the data plane bearer instead of exchanging a service principal.
    pub fn with_auth_bypass(mut self, token: ShortLivedToken) -> Self {
        self.auth_bypass = Some(token);
        self
    }

    /// Ties the provider to the worker it serves. Binding the same worker again is allowed.
    pub fn bind_worker(&self, worker: &WorkerId) -> Result<(), WorkerBindingError> {
        (self.worker_id.get_or_init(|| worker.clone()) == worker)
            .then_some(())
            .ok_or(WorkerBindingError::AlreadyBound)
    }

    /// How many leases still hold a session.
    pub async fn active_lease_count(&self) -> usize {
        self.active.lock().await.len()
    }

    pub(crate) fn lease_gate(&self, lease: &LeaseId) -> Arc<Mutex<()>> {
        let mut gates = self.gates.lock().unwrap_or_else(PoisonError::into_inner);
        gates.retain(|_, gate| gate.strong_count() > 0);
        gates.get(lease).and_then(Weak::upgrade).unwrap_or_else(|| {
            let gate = Arc::new(Mutex::new(()));
            gates.insert(lease.clone(), Arc::downgrade(&gate));
            gate
        })
    }

    pub(crate) async fn holds(&self, lease: &LeaseId, resource: &AuthorityResourceId) -> bool {
        self.active
            .lock()
            .await
            .get(lease)
            .is_some_and(|held| &held.resource == resource)
    }

    async fn bearer(&self, tenant: &TenantId, ttl_seconds: NonZeroU32) -> Result<ShortLivedToken, AcaError> {
        if let Some(token) = &self.auth_bypass {
            return Ok(token.clone());
        }
        let issued = self
            .issuer
            .issue(
                CredentialProvider::Azure,
                tenant,
                &self.config.azure_credential_id,
                CredentialScope::AzureServicePrincipal {
                    resource_id: self.config.management_endpoint.to_string(),
                },
                ttl_seconds,
            )
            .await?;
        let principal = ServicePrincipal::parse(&issued)?;
        fetch_dynamic_sessions_token(&self.http, &self.config.login_endpoint, &principal).await
    }

    async fn lease_inner(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, AcaError> {
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        if self.active.lock().await.contains_key(lease_id) {
            return Err(AcaError::InvalidSpec(
                "this lease still holds a native resource; settle cleanup before reallocating".to_owned(),
            ));
        }
        let lease_sandbox = SandboxId::new(lease_id.as_str())?;
        let emitter = (self.emitters)(lease_id);
        emitter
            .emit(LifecycleEvent::Started {
                sandbox_id: lease_sandbox.clone(),
            })
            .await;
        let outcome = run_phase(Some(emitter.as_ref()), SandboxPhase::Allocate, None, || {
            self.allocate_locked(req, lease_id)
        })
        .await;
        let outcome = match outcome {
            Ok(lease) => Ok(lease),
            Err(primary) => Err(self.abandon_allocation(lease_id, primary).await),
        };
        emitter
            .emit(match &outcome {
                Ok(lease) => LifecycleEvent::Completed {
                    sandbox_id: lease.sandbox_id.clone(),
                },
                Err(error) => LifecycleEvent::Failed {
                    sandbox_id: lease_sandbox,
                    error: error.to_string(),
                },
            })
            .await;
        outcome
    }

    async fn abandon_allocation(&self, lease_id: &LeaseId, primary: AcaError) -> AcaError {
        if let Some(held) = self.active.lock().await.get_mut(lease_id) {
            held.cleanup_requested = true;
        }
        match self.cleanup_locked(lease_id).await {
            Ok(_) => primary,
            Err(cleanup) => AcaError::AllocationCleanup {
                primary: Box::new(primary),
                cleanup: Box::new(cleanup),
            },
        }
    }

    async fn allocate_locked(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, AcaError> {
        if !req.spec.env().is_empty() {
            return Err(AcaError::InvalidSpec(
                "spec.env is not supported on the ACA provider".to_owned(),
            ));
        }
        if !req.spec.mounts().is_empty() {
            return Err(AcaError::InvalidSpec(
                "SandboxSpec.mounts (RemoteMount) is not supported on the ACA provider: Dynamic Sessions' \
                 data-plane API has no volume/mount primitive, so the profile offers no FUSE mounts"
                    .to_owned(),
            ));
        }
        let image = resolve_registry_image(req.spec.image(), self.config.default_image.as_ref())?;
        let credential_ttl = credential_ttl_seconds(
            req.ttl_seconds,
            req.spec.no_ttl().is_unbounded(),
            self.max_lease_lifetime,
        );
        let bearer = self.bearer(&req.tenant, credential_ttl).await?;

        let session = SessionIdentifier::generate();
        let (resource, completion) = self.scope.track(lease_id.clone(), session.as_str())?;
        self.active.lock().await.insert(
            lease_id.clone(),
            ActiveAcaLease {
                tenant: req.tenant.clone(),
                session: session.clone(),
                resource,
                completion,
                workspace: None,
                volume_synced: false,
                cleanup_requested: false,
            },
        );
        wait_session_ready(
            self.api.as_ref(),
            &bearer,
            &session,
            SESSION_READY_ATTEMPTS,
            SESSION_READY_POLL_INTERVAL,
        )
        .await?;

        let workspace = Arc::new(materialize_workspace(req.spec.workspace(), &req.tenant, &self.volumes).await?);
        if let Some(held) = self.active.lock().await.get_mut(lease_id) {
            held.workspace = Some(Arc::clone(&workspace));
        }
        let uploaded = upload_directory(self.api.as_ref(), &bearer, &session, workspace.local_path()).await?;
        info!(
            %lease_id,
            pool = %self.config.session_pool_name,
            uploaded_files = uploaded,
            image = %image,
            "ACA session workspace uploaded"
        );

        for (key, value) in image_env_vars(&image) {
            let setup = format!("import os; os.environ[{key:?}] = {value:?}");
            self.api.execute_code(&bearer, &session, &setup, "python").await?;
        }

        Ok(ProviderLease {
            epoch: None,
            sandbox_id: SandboxId::new(session.as_str())?,
            evidence_subject: evidence_subject(lease_id),
            worker_id: self.worker_id.get().cloned(),
            attempt_id: None,
        })
    }

    /// Retries the cleanup of every allocation that failed and left its session behind.
    pub async fn retry_failed_allocations(&self) {
        let pending: Vec<LeaseId> = self
            .active
            .lock()
            .await
            .iter()
            .filter(|(_, held)| held.cleanup_requested)
            .map(|(lease, _)| lease.clone())
            .collect();
        for lease in pending {
            let gate = self.lease_gate(&lease);
            let _held = gate.lock().await;
            if let Err(error) = self.cleanup_locked(&lease).await {
                warn!(lease_id = %lease, %error, "allocation cleanup retry failed");
            }
        }
    }

    pub(crate) async fn cleanup_locked(&self, lease_id: &LeaseId) -> Result<SessionDeleteOutcome, AcaError> {
        let Some(active) = self.active.lock().await.get(lease_id).cloned() else {
            return Ok(SessionDeleteOutcome::Absent);
        };
        if !active.volume_synced {
            if let Some(workspace) = &active.workspace
                && let Some(plan) = workspace.volume_sync()
            {
                self.volumes.push(plan, &active.tenant, workspace.local_path()).await?;
            }
            if let Some(held) = self.active.lock().await.get_mut(lease_id) {
                held.volume_synced = true;
            }
        }
        let bearer = self.bearer(&active.tenant, CLEANUP_CREDENTIAL_TTL_SECONDS).await?;
        let outcome = self.api.delete_session(&bearer, &active.session).await?;
        self.active.lock().await.remove(lease_id);
        active.completion.confirm();
        Ok(outcome)
    }
}

#[async_trait]
impl Provider for AcaWorkerProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::AzureContainerApps
    }

    async fn lease(&self, req: &LeaseRequest, lease_id: &LeaseId) -> Result<ProviderLease, ProviderError> {
        reject_unsupported_privileged(&req.spec, &PROFILE)?;
        self.lease_inner(req, lease_id).await.map_err(ProviderError::backend)
    }

    async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ProviderError> {
        let gate = self.lease_gate(lease_id);
        let _held = gate.lock().await;
        self.cleanup_locked(lease_id)
            .await
            .map(|_| ())
            .map_err(ProviderError::backend)
    }
}
