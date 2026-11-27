use super::*;

pub struct InMemoryCommandLaunchStore {
    max_age: Duration,
    journal: Mutex<InMemoryLaunchJournal>,
}

#[derive(Default)]
struct InMemoryLaunchJournal {
    launches: HashMap<(String, String), (CommandLaunch, Instant)>,
    authorities: HashMap<(String, String), CommandCancellationAuthority>,
}

impl Default for InMemoryCommandLaunchStore {
    fn default() -> Self {
        Self::with_max_age(LAUNCH_KEY_MAX_AGE)
    }
}

impl InMemoryCommandLaunchStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Expires each key `max_age` after its last write, like the KV bucket's
    /// per-message max age.
    pub fn with_max_age(max_age: Duration) -> Self {
        Self {
            max_age,
            journal: Mutex::new(InMemoryLaunchJournal::default()),
        }
    }

    async fn live(&self) -> tokio::sync::MutexGuard<'_, InMemoryLaunchJournal> {
        let mut journal = self.journal.lock().await;
        let max_age = self.max_age;
        journal.launches.retain(|_, (_, written)| written.elapsed() <= max_age);
        journal
    }
}

#[async_trait]
impl CommandLaunchStore for InMemoryCommandLaunchStore {
    fn cancellation_custody_support(&self) -> CancellationCustodySupport {
        CancellationCustodySupport::Retained
    }

    async fn claim(&self, lease: &LeaseId, key: &LaunchKey, fingerprint: &str) -> Result<Claim, CommandLaunchError> {
        let mut journal = self.live().await;
        let slot = (lease.as_str().to_string(), key.as_str().to_string());
        if let Some((existing, _)) = journal.launches.get(&slot) {
            return Ok(Claim::Existing(existing.clone()));
        }
        journal.launches.insert(
            slot,
            (CommandLaunch::new(fingerprint, LaunchState::Pending), Instant::now()),
        );
        Ok(Claim::Claimed)
    }

    async fn retain_cancellation_authority(
        &self,
        lease: &LeaseId,
        key: &LaunchKey,
        authority: &CommandCancellationAuthority,
    ) -> Result<CancellationAuthorityClaim, CommandLaunchError> {
        authority.validate(lease, key)?;
        let mut journal = self.live().await;
        let slot = (lease.as_str().to_string(), key.as_str().to_string());
        if let Some(existing) = journal.authorities.get(&slot) {
            existing.validate(lease, key)?;
            return Ok(CancellationAuthorityClaim::Existing(existing.clone()));
        }
        let (launch, _) = journal
            .launches
            .get(&slot)
            .ok_or(CommandLaunchError::LaunchUnavailable)?;
        if !(launch.fingerprint == authority.fingerprint && matches!(launch.state, LaunchState::Pending)) {
            return Err(CommandLaunchError::AuthorityMismatch);
        }
        journal.authorities.insert(slot, authority.clone());
        Ok(CancellationAuthorityClaim::New)
    }

    async fn cancellation_authority(
        &self,
        lease: &LeaseId,
        key: &LaunchKey,
    ) -> Result<Option<CommandCancellationAuthority>, CommandLaunchError> {
        let journal = self.live().await;
        let slot = (lease.as_str().to_string(), key.as_str().to_string());
        let Some(authority) = journal.authorities.get(&slot) else {
            return Ok(None);
        };
        authority.validate(lease, key)?;
        Ok(Some(authority.clone()))
    }

    async fn finish(&self, lease: &LeaseId, key: &LaunchKey, launch: &CommandLaunch) -> Result<(), CommandLaunchError> {
        self.live().await.launches.insert(
            (lease.as_str().to_string(), key.as_str().to_string()),
            (launch.clone(), Instant::now()),
        );
        Ok(())
    }

    async fn forget(&self, lease: &LeaseId, key: &LaunchKey) -> Result<(), CommandLaunchError> {
        let mut journal = self.live().await;
        let slot = (lease.as_str().to_string(), key.as_str().to_string());
        journal.launches.remove(&slot);
        Ok(())
    }

    async fn replace(
        &self,
        lease: &LeaseId,
        key: &LaunchKey,
        expected: &CommandLaunch,
        next: &CommandLaunch,
    ) -> Result<bool, CommandLaunchError> {
        let mut journal = self.live().await;
        match journal
            .launches
            .get_mut(&(lease.as_str().to_string(), key.as_str().to_string()))
        {
            Some(current) if current.0 == *expected => {
                *current = (next.clone(), Instant::now());
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    async fn forget_lease(&self, lease: &LeaseId) -> Result<(), CommandLaunchError> {
        let mut journal = self.live().await;
        journal.launches.retain(|(owner, _), _| owner != lease.as_str());
        Ok(())
    }
}
