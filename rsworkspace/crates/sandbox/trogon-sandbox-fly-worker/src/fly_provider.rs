use std::collections::{BTreeMap, HashMap};
use std::fs::{File, OpenOptions};
use std::sync::Arc;

use tokio::sync::Mutex;
use trogon_sandbox_contract::{
    DriveMount, DriveMountMode, LeaseId, NativeVolumeBinding, Port, ProviderCapabilityProfile, ProviderKind,
    ProviderLease, ProviderVolumeRef, SandboxId, WorkerId,
};
use trogon_sandbox_protocol::{AttemptLedger, subjects};

use crate::constants::{LEDGER_FILE, OWNER_FILE, STATE_LOCK_FILE};
use crate::{FlyApi, FlyConfig, FlyProviderError, FlyVolumeId, MachineId, MachineSize, profile};

/// What the worker remembers about one held lease.
#[derive(Clone)]
pub(crate) struct LeaseRecord {
    pub(crate) machine: MachineId,
    pub(crate) cwd: String,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) exposed_ports: Vec<Port>,
    pub(crate) workspace_volume: Option<FlyVolumeId>,
}

/// Leases Fly Machines, one per sandbox, and keeps them recoverable across restarts.
pub struct FlyProvider {
    pub(crate) api: FlyApi,
    pub(crate) config: FlyConfig,
    pub(crate) owner: String,
    pub(crate) active: Mutex<HashMap<LeaseId, LeaseRecord>>,
    pub(crate) warm: Mutex<Vec<MachineId>>,
    pub(crate) allocation: Mutex<()>,
    pub(crate) ledger: Arc<AttemptLedger>,
    _state_lock: File,
}

fn read_owner(state_dir: &std::path::Path) -> Result<String, FlyProviderError> {
    let path = state_dir.join(OWNER_FILE);
    let owner = match std::fs::read_to_string(&path) {
        Ok(owner) => owner,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let owner = uuid::Uuid::new_v4().to_string();
            std::fs::write(&path, &owner)?;
            owner
        }
        Err(error) => return Err(error.into()),
    };
    uuid::Uuid::parse_str(&owner)?;
    Ok(owner)
}

impl FlyProvider {
    /// Takes the state directory, learns this worker's identity from it, and opens its dispatch ledger.
    pub fn new(api: FlyApi, config: FlyConfig) -> Result<Self, FlyProviderError> {
        std::fs::create_dir_all(&config.state_dir)?;
        #[cfg(unix)]
        std::fs::set_permissions(&config.state_dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
        let state_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(config.state_dir.join(STATE_LOCK_FILE))?;
        state_lock.try_lock().map_err(|_| FlyProviderError::StateLocked)?;
        let owner = read_owner(&config.state_dir)?;
        let worker_id = WorkerId::new(format!("fly-{owner}"))?;
        let ledger = Arc::new(AttemptLedger::open(
            config.state_dir.join(LEDGER_FILE),
            ProviderKind::Fly,
            Some(worker_id),
        )?);
        Ok(Self {
            api,
            config,
            owner,
            active: Mutex::new(HashMap::new()),
            warm: Mutex::new(Vec::new()),
            allocation: Mutex::new(()),
            ledger,
            _state_lock: state_lock,
        })
    }

    pub fn config(&self) -> &FlyConfig {
        &self.config
    }

    pub fn api(&self) -> &FlyApi {
        &self.api
    }

    /// The durable record of dispatch attempts this worker answers to.
    pub fn dispatch_ledger(&self) -> &Arc<AttemptLedger> {
        &self.ledger
    }

    pub async fn current_leases(&self) -> usize {
        self.active.lock().await.len()
    }

    pub async fn holds_lease(&self, id: &LeaseId) -> bool {
        self.active.lock().await.contains_key(id)
    }

    pub(crate) async fn record(&self, id: &LeaseId) -> Result<LeaseRecord, FlyProviderError> {
        self.active
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or(FlyProviderError::NotHeld)
    }

    pub(crate) fn default_size(&self) -> MachineSize {
        MachineSize::exact(self.config.cpus, self.config.memory_mib)
    }

    pub(crate) fn receipt(machine: &MachineId, lease: &LeaseId) -> Result<ProviderLease, FlyProviderError> {
        Ok(ProviderLease {
            sandbox_id: SandboxId::new(machine.as_str())?,
            evidence_subject: subjects::evidence_subject(lease),
            worker_id: None,
            attempt_id: None,
            epoch: None,
        })
    }

    /// The drive this worker's operator volume serves, as the heartbeat advertises it.
    pub fn native_volume_bindings(&self) -> Vec<NativeVolumeBinding> {
        self.config
            .volume
            .iter()
            .map(|binding| NativeVolumeBinding {
                mount: DriveMount {
                    drive_id: binding.drive_id().clone(),
                    mount_path: binding.mount_path().clone(),
                    mode: DriveMountMode::ReadWrite,
                },
                storage: ProviderVolumeRef {
                    provider: ProviderKind::Fly,
                    volume_id: binding.provider_volume_id().clone(),
                },
            })
            .collect()
    }

    /// The capabilities this worker offers with its current settings.
    pub fn profile(&self) -> ProviderCapabilityProfile {
        profile::capability_profile(&self.config)
    }
}
