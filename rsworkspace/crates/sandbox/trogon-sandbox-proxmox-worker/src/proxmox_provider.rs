use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::sync::Arc;

use tokio::sync::Mutex;
use trogon_sandbox_contract::{LeaseId, ProviderCapabilityProfile, ProviderKind, ProviderLease, SandboxId, WorkerId};
use trogon_sandbox_protocol::{AttemptLedger, subjects};

use crate::constants::{LEDGER_FILE, OWNER_FILE, STATE_LOCK_FILE};
use crate::{LeaseProxy, LeaseRecord, ProxmoxApi, ProxmoxConfig, ProxmoxProviderError, VmId, profile};

/// Leases Proxmox VMs, one linked clone per sandbox, and keeps them recoverable across restarts.
pub struct ProxmoxProvider {
    pub(crate) api: ProxmoxApi,
    pub(crate) config: ProxmoxConfig,
    pub(crate) active: Mutex<HashMap<LeaseId, LeaseRecord>>,
    pub(crate) proxies: Mutex<HashMap<LeaseId, LeaseProxy>>,
    pub(crate) pending_cleanup: Mutex<VecDeque<LeaseId>>,
    pub(crate) allocation: Mutex<()>,
    pub(crate) ledger: Arc<AttemptLedger>,
    _state_lock: File,
}

fn read_owner(state_dir: &std::path::Path) -> Result<String, ProxmoxProviderError> {
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

impl ProxmoxProvider {
    /// Takes the state directory, learns this worker's identity from it, and opens its dispatch ledger.
    pub fn new(api: ProxmoxApi, config: ProxmoxConfig) -> Result<Self, ProxmoxProviderError> {
        std::fs::create_dir_all(&config.state_dir)?;
        #[cfg(unix)]
        std::fs::set_permissions(&config.state_dir, std::os::unix::fs::PermissionsExt::from_mode(0o700))?;
        let state_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(config.state_dir.join(STATE_LOCK_FILE))?;
        state_lock.try_lock().map_err(|_| ProxmoxProviderError::StateLocked)?;
        let owner = read_owner(&config.state_dir)?;
        let worker_id = WorkerId::new(format!("proxmox-{owner}"))?;
        let ledger = Arc::new(AttemptLedger::open(
            config.state_dir.join(LEDGER_FILE),
            ProviderKind::Proxmox,
            Some(worker_id),
        )?);
        Ok(Self {
            api,
            config,
            active: Mutex::new(HashMap::new()),
            proxies: Mutex::new(HashMap::new()),
            pending_cleanup: Mutex::new(VecDeque::new()),
            allocation: Mutex::new(()),
            ledger,
            _state_lock: state_lock,
        })
    }

    pub fn config(&self) -> &ProxmoxConfig {
        &self.config
    }

    pub fn api(&self) -> &ProxmoxApi {
        &self.api
    }

    /// The durable record of dispatch attempts this worker answers to.
    pub fn dispatch_ledger(&self) -> &Arc<AttemptLedger> {
        &self.ledger
    }

    pub async fn holds_lease(&self, lease: &LeaseId) -> bool {
        self.active.lock().await.contains_key(lease)
    }

    pub(crate) async fn record(&self, lease: &LeaseId) -> Result<LeaseRecord, ProxmoxProviderError> {
        self.active
            .lock()
            .await
            .get(lease)
            .cloned()
            .ok_or(ProxmoxProviderError::NotHeld)
    }

    pub(crate) fn receipt(vm: VmId, lease: &LeaseId) -> Result<ProviderLease, ProxmoxProviderError> {
        Ok(ProviderLease {
            sandbox_id: SandboxId::new(vm.to_string())?,
            evidence_subject: subjects::evidence_subject(lease),
            worker_id: None,
            attempt_id: None,
            epoch: None,
        })
    }

    /// The capabilities this worker offers with its current settings.
    pub fn profile(&self) -> ProviderCapabilityProfile {
        profile::capability_profile(&self.config)
    }
}
