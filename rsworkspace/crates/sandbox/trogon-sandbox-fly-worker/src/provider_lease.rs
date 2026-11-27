use std::time::Duration;

use trogon_sandbox_contract::{
    LeaseAttemptIdentity, LeaseId, LeaseRequest, ProviderDispatchPayload, ProviderKind, ProviderLease,
};

use crate::check::ensure;
use crate::constants::{DEFAULT_CWD, META_ATTEMPT, META_LEASE};
use crate::fly_provider::LeaseRecord;
use crate::{FlyProvider, FlyProviderError, MachineId, MachineSize};

impl FlyProvider {
    /// Destroys every Machine and workspace volume this worker owns. Returns how many Machines were destroyed.
    pub async fn recover(&self) -> Result<usize, FlyProviderError> {
        let _guard = self.allocation.lock().await;
        let mut count = 0;
        for machine in self.api.list().await? {
            if self.owned(&machine) {
                for state in self.ledger.try_states().await? {
                    if machine.metadata(META_ATTEMPT) == Some(state.identity.target.attempt_id.as_str())
                        && machine.metadata(META_LEASE) == Some(state.identity.lease_id.as_str())
                        && state.phase != trogon_sandbox_protocol::AttemptPhase::Cleaned
                    {
                        self.record_machine(&machine.id, Some(&state.identity)).await?;
                    }
                }
                self.destroy_owned(&machine.id).await?;
                count += 1;
            }
        }
        if self.config.workspace_volume.is_some() {
            self.sweep_workspace_volumes().await?;
        }
        self.active.lock().await.clear();
        self.warm.lock().await.clear();
        Ok(count)
    }

    pub async fn shutdown(&self) -> Result<(), FlyProviderError> {
        self.recover().await.map(|_| ())
    }

    pub(crate) async fn lease_inner(
        &self,
        request: &LeaseRequest,
        lease: &LeaseId,
        identity: Option<&LeaseAttemptIdentity>,
    ) -> Result<ProviderLease, FlyProviderError> {
        let volume = self.validate_lease(request)?;
        let restore = self.restore_source(request.spec.image())?;
        let spec = &request.spec;
        let _guard = self.allocation.lock().await;
        let free = !self.holds_lease(lease).await && self.current_leases().await < self.config.max_leases;
        ensure(free, FlyProviderError::Capacity)?;
        let size = MachineSize::fit(spec.vcpu_millis(), spec.memory_mib(), self.config.cpus)?;
        // Warm Machines boot without services, so a lease that publishes ports needs a fresh Machine.
        let warm = if size == self.default_size() && spec.exposed_ports().is_empty() {
            self.warm.lock().await.pop()
        } else {
            None
        };
        let cwd = spec.working_dir().unwrap_or(DEFAULT_CWD).to_owned();
        let workspace_volume = match &self.config.workspace_volume {
            Some(settings) => Some(self.create_workspace_volume(settings, restore.as_deref()).await?),
            None => None,
        };
        let machine = match warm {
            Some(id) => self.start_warm(&id, identity).await?,
            None => {
                let mount = workspace_volume.as_ref().map(|id| (id, cwd.as_str()));
                match self
                    .create(size, false, volume, mount, spec.exposed_ports(), identity)
                    .await
                {
                    Ok(id) => id,
                    Err(error) => {
                        self.discard_workspace_volume(workspace_volume.as_ref()).await;
                        return Err(error);
                    }
                }
            }
        };
        if let Err(error) = self.prepare_workspace(&machine, &cwd).await {
            self.destroy_owned(&machine).await?;
            return Err(error);
        }
        let mut env = spec.env().clone();
        for name in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            if let Some(proxy) = &self.config.egress_proxy {
                env.insert(name.into(), proxy.clone());
            }
        }
        let receipt = Self::receipt(&machine, lease)?;
        self.active.lock().await.insert(
            lease.clone(),
            LeaseRecord {
                machine,
                cwd,
                env,
                exposed_ports: spec.exposed_ports().to_vec(),
                workspace_volume,
            },
        );
        Ok(receipt)
    }

    async fn start_warm(
        &self,
        id: &MachineId,
        identity: Option<&LeaseAttemptIdentity>,
    ) -> Result<MachineId, FlyProviderError> {
        let current = self
            .api
            .get(id)
            .await?
            .ok_or(FlyProviderError::Backend("warm Machine disappeared"))?;
        ensure(self.owned(&current), FlyProviderError::Foreign("the warm Machine"))?;
        self.record_machine(id, identity).await?;
        if let Err(error) = self.api.start(id).await {
            self.destroy_owned(id).await?;
            return Err(error.into());
        }
        Ok(id.clone())
    }

    async fn prepare_workspace(&self, machine: &MachineId, cwd: &str) -> Result<(), FlyProviderError> {
        self.ready(machine).await?;
        let command = vec!["/bin/mkdir".into(), "-p".into(), "--".into(), cwd.to_owned()];
        let output = self.api.exec(machine, command, Duration::from_secs(10)).await?;
        ensure(
            output.exit_code == 0,
            FlyProviderError::Backend("workspace setup failed"),
        )
    }

    /// Runs a dispatch that carries the assignment the broker made, through the durable ledger.
    pub async fn lease_dispatch(&self, dispatch: &ProviderDispatchPayload) -> Result<ProviderLease, FlyProviderError> {
        let request = &dispatch.request;
        ensure(
            dispatch.resolved_drives.len() == request.spec.drives().len(),
            FlyProviderError::Rejected("native volume requests require exact registered drive records"),
        )?;
        for mount in request.spec.drives() {
            let binding = self
                .config
                .volume
                .as_ref()
                .ok_or(FlyProviderError::Rejected("native volume binding is missing"))?;
            let record = dispatch
                .resolved_drives
                .iter()
                .find(|record| record.id == mount.drive_id)
                .ok_or(FlyProviderError::Rejected("registered drive record is missing"))?;
            let volume = record.storage.provider_volume().ok_or(FlyProviderError::Rejected(
                "S3 drives cannot be reinterpreted as Fly volumes",
            ))?;
            let authentic = record.tenant == request.tenant
                && record.id.tenant == request.tenant
                && record.id == *binding.drive_id()
                && volume.provider == ProviderKind::Fly
                && volume.volume_id == *binding.provider_volume_id();
            ensure(
                authentic,
                FlyProviderError::Rejected("registered native volume authority does not match the requested binding"),
            )?;
        }
        self.validate_lease(request)?;
        let Some(target) = dispatch.target.clone() else {
            return self.lease_inner(request, &dispatch.lease_id, None).await;
        };
        let identity = LeaseAttemptIdentity {
            lease_id: dispatch.lease_id.clone(),
            provider: ProviderKind::Fly,
            target,
        };
        self.ledger
            .dispatch(&identity, || {
                self.lease_inner(request, &dispatch.lease_id, Some(&identity))
            })
            .await
            .map_err(FlyProviderError::from)
    }
}
