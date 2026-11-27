use std::collections::BTreeMap;

use trogon_sandbox_contract::{LeaseAttemptIdentity, LeaseId, Port, ProviderCleanupRequest, ProviderCleanupResponse};

use crate::check::ensure;
use crate::constants::{META_ATTEMPT, META_LEASE, META_NAME, META_OWNER, META_WORKSPACE_VOLUME, POLL};
use crate::create_requests::{
    CreateMachineBody, GuestBody, InitBody, MachineConfigBody, MountBody, PortBody, RestartBody, ServiceBody,
};
use crate::{
    FlyApiError, FlyPreviewNetwork, FlyProvider, FlyProviderError, FlyVolumeBinding, FlyVolumeId, Machine, MachineId,
    MachineSize, MachineState,
};

impl FlyProvider {
    pub(crate) fn owned(&self, machine: &Machine) -> bool {
        machine.metadata(META_OWNER) == Some(self.owner.as_str())
    }

    pub(crate) async fn destroy_owned(&self, id: &MachineId) -> Result<(), FlyProviderError> {
        let Some(machine) = self.api.get(id).await? else {
            return Ok(());
        };
        ensure(self.owned(&machine), FlyProviderError::Foreign("the Machine"))?;
        self.api.destroy(id).await?;
        if let Some(volume) = machine.metadata(META_WORKSPACE_VOLUME) {
            self.delete_workspace_volume(&FlyVolumeId::new(volume)?).await?;
        }
        Ok(())
    }

    pub(crate) async fn record_machine(
        &self,
        id: &MachineId,
        identity: Option<&LeaseAttemptIdentity>,
    ) -> Result<(), FlyProviderError> {
        if let Some(identity) = identity {
            self.ledger
                .record_backend_receipt(identity, Self::receipt(id, &identity.lease_id)?)
                .await?;
        }
        Ok(())
    }

    /// Creates a Machine and returns its id once ownership and the durable receipt are in place.
    pub(crate) async fn create(
        &self,
        size: MachineSize,
        skip_launch: bool,
        volume: Option<&FlyVolumeBinding>,
        workspace: Option<(&FlyVolumeId, &str)>,
        exposed_ports: &[Port],
        identity: Option<&LeaseAttemptIdentity>,
    ) -> Result<MachineId, FlyProviderError> {
        let name = format!("trogon-sandbox-{}", uuid::Uuid::new_v4().simple());
        let handlers: &'static [&'static str] = match self.config.preview_network {
            FlyPreviewNetwork::Public => &["tls", "http"],
            FlyPreviewNetwork::Flycast => &["http"],
        };
        let services: Vec<_> = exposed_ports
            .iter()
            .map(|port| ServiceBody {
                protocol: "tcp",
                internal_port: port.get(),
                autostop: "off",
                autostart: false,
                ports: [PortBody {
                    port: port.get(),
                    handlers,
                }],
            })
            .collect();
        let mut metadata = BTreeMap::new();
        metadata.insert(META_OWNER, self.owner.as_str());
        metadata.insert(META_NAME, name.as_str());
        if let Some(identity) = identity {
            metadata.insert(META_ATTEMPT, identity.target.attempt_id.as_str());
            metadata.insert(META_LEASE, identity.lease_id.as_str());
        }
        if let Some((volume, _)) = workspace {
            metadata.insert(META_WORKSPACE_VOLUME, volume.as_str());
        }
        let mut config = MachineConfigBody {
            image: self.config.image.as_str(),
            init: InitBody {
                exec: ["/bin/sleep", "infinity"],
            },
            guest: GuestBody {
                cpus: size.cpus(),
                cpu_kind: "shared",
                memory_mb: size.memory_mib(),
            },
            restart: RestartBody { policy: "no" },
            auto_destroy: false,
            metadata,
            services,
            mounts: None,
        };
        if let Some(binding) = volume {
            let details = self.api.volume(binding.volume_id()).await?;
            let free =
                details.attached_machine_id.is_none() && details.region.as_deref() == Some(self.config.region.as_str());
            ensure(
                free,
                FlyProviderError::Backend("volume is attached or in a different region"),
            )?;
            config.mounts = Some([MountBody {
                volume: binding.volume_id().as_str(),
                path: binding.mount_path().as_str(),
            }]);
        }
        if let Some((volume, path)) = workspace {
            config.mounts = Some([MountBody {
                volume: volume.as_str(),
                path,
            }]);
        }
        let body = CreateMachineBody {
            name: name.as_str(),
            region: self.config.region.as_str(),
            skip_launch,
            config,
        };
        let body = serde_json::to_value(&body).map_err(FlyApiError::Decode)?;
        match self.api.create(&body).await {
            Ok(machine) => {
                ensure(self.owned(&machine), FlyProviderError::Foreign("the created Machine"))?;
                if let Err(error) = self.record_machine(&machine.id, identity).await {
                    self.destroy_owned(&machine.id).await?;
                    return Err(error);
                }
                Ok(machine.id)
            }
            Err(error) => {
                // A timed-out create may have succeeded; reclaim only this attempt's name.
                for machine in self.api.list().await? {
                    if self.owned(&machine) && machine.metadata(META_NAME) == Some(name.as_str()) {
                        self.record_machine(&machine.id, identity).await?;
                        self.destroy_owned(&machine.id).await?;
                    }
                }
                Err(error.into())
            }
        }
    }

    /// Tops the warm pool up to its configured size, unless a lease is being allocated.
    pub async fn maintain_pool(&self) -> Result<(), FlyProviderError> {
        let Ok(_guard) = self.allocation.try_lock() else {
            return Ok(());
        };
        while self.warm.lock().await.len() < self.config.warm_pool_size {
            let machine = self.create(self.default_size(), true, None, None, &[], None).await?;
            self.warm.lock().await.push(machine);
        }
        Ok(())
    }

    pub(crate) async fn ready(&self, id: &MachineId) -> Result<(), FlyProviderError> {
        tokio::time::timeout(self.config.boot_timeout, async {
            loop {
                let machine = self
                    .api
                    .get(id)
                    .await?
                    .ok_or(FlyProviderError::Backend("Machine disappeared during boot"))?;
                ensure(self.owned(&machine), FlyProviderError::Foreign("the booting Machine"))?;
                match machine.state {
                    MachineState::Started => return Ok(()),
                    MachineState::Destroyed | MachineState::Failed => {
                        return Err(FlyProviderError::Backend("Machine failed to start"));
                    }
                    _ => tokio::time::sleep(POLL).await,
                }
            }
        })
        .await
        .map_err(|_| FlyProviderError::TimedOut("Machine boot"))?
    }

    async fn machine_state(&self, id: &MachineId) -> Result<MachineState, FlyProviderError> {
        let machine = self
            .api
            .get(id)
            .await?
            .ok_or(FlyProviderError::Backend("Machine is missing"))?;
        ensure(self.owned(&machine), FlyProviderError::Foreign("the Machine"))?;
        Ok(machine.state)
    }

    pub(crate) async fn suspend(&self, id: &MachineId) -> Result<(), FlyProviderError> {
        if self.machine_state(id).await? == MachineState::Suspended {
            return Ok(());
        }
        self.api.suspend(id).await?;
        let settled = tokio::time::timeout(self.config.boot_timeout, async {
            loop {
                let state = self.machine_state(id).await?;
                if matches!(state, MachineState::Suspended | MachineState::Stopped) {
                    return Ok::<_, FlyProviderError>(state);
                }
                tokio::time::sleep(POLL).await;
            }
        })
        .await
        .map_err(|_| FlyProviderError::TimedOut("Machine suspend"))??;
        if settled == MachineState::Stopped {
            // Fly stops a Machine it cannot snapshot. Its processes are gone, so boot it again rather than leave
            // a lease the broker would resume into an empty guest.
            self.api.start(id).await?;
            self.ready(id).await?;
            return Err(FlyProviderError::Backend(
                "Fly stopped the Machine instead of suspending it; it was restarted",
            ));
        }
        Ok(())
    }

    pub(crate) async fn resume(&self, id: &MachineId) -> Result<(), FlyProviderError> {
        if self.machine_state(id).await? == MachineState::Started {
            return Ok(());
        }
        self.api.start(id).await?;
        self.ready(id).await
    }

    /// Tears down the attempt a cleanup request names, wherever its Machine is.
    pub async fn cleanup_dispatch(&self, request: &ProviderCleanupRequest) -> ProviderCleanupResponse {
        self.ledger
            .cleanup(request, |state| async move {
                if state.backend_cleaned {
                    return Ok(());
                }
                let _guard = self.allocation.lock().await;
                let id = match state.receipt {
                    Some(receipt) => MachineId::new(receipt.sandbox_id.as_str())?,
                    None => self.find_attempt_machine(&state.identity).await?,
                };
                if let Some(active) = self.active.lock().await.get(&state.identity.lease_id) {
                    ensure(active.machine == id, FlyProviderError::DifferentAttempt)?;
                }
                self.destroy_owned(&id).await?;
                self.active.lock().await.remove(&state.identity.lease_id);
                Ok::<_, FlyProviderError>(())
            })
            .await
    }

    async fn find_attempt_machine(&self, identity: &LeaseAttemptIdentity) -> Result<MachineId, FlyProviderError> {
        let matches: Vec<Machine> = self
            .api
            .list()
            .await?
            .into_iter()
            .filter(|machine| {
                self.owned(machine)
                    && machine.metadata(META_ATTEMPT) == Some(identity.target.attempt_id.as_str())
                    && machine.metadata(META_LEASE) == Some(identity.lease_id.as_str())
            })
            .collect();
        let [machine] = matches.as_slice() else {
            return Err(FlyProviderError::Ambiguous);
        };
        self.record_machine(&machine.id, Some(identity)).await?;
        Ok(machine.id.clone())
    }

    pub(crate) async fn forget(&self, lease: &LeaseId) {
        self.active.lock().await.remove(lease);
    }
}
