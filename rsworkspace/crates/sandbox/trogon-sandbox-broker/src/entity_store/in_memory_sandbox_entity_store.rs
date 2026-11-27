use super::*;

#[derive(Default)]
struct MemoryState {
    entities: HashMap<(TenantId, SandboxEntityId), (SandboxEntityRecord, u64)>,
    names: HashMap<(TenantId, String), SandboxEntityId>,
}

#[derive(Default)]
pub struct InMemorySandboxEntityStore {
    state: Mutex<MemoryState>,
}

impl InMemorySandboxEntityStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Leaves a name claim whose entity does not exist, as a crash between
    /// delete steps would.
    #[cfg(test)]
    pub(crate) fn insert_dangling_name(&self, tenant: &TenantId, name: &str) -> Result<(), EntityStoreError> {
        let id = new_entity_id()?;
        self.lock().names.insert((tenant.clone(), name.to_string()), id);
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[async_trait]
impl SandboxEntityStore for InMemorySandboxEntityStore {
    async fn create(&self, new: NewSandboxEntity) -> Result<Versioned, EntityStoreError> {
        if let Some(name) = &new.name {
            validate_name(name)?;
        }
        let record = build_record(new, new_entity_id()?)?;
        let mut state = self.lock();
        if let Some(name) = &record.name {
            let key = (record.tenant.clone(), name.clone());
            if state.names.contains_key(&key) {
                return Err(EntityStoreError::NameTaken);
            }
            state.names.insert(key, record.entity_id.clone());
        }
        state
            .entities
            .insert((record.tenant.clone(), record.entity_id.clone()), (record.clone(), 1));
        Ok(Versioned { record, revision: 1 })
    }

    async fn get(&self, tenant: &TenantId, id: &SandboxEntityId) -> Result<Option<Versioned>, EntityStoreError> {
        Ok(self
            .lock()
            .entities
            .get(&(tenant.clone(), id.clone()))
            .map(|(record, revision)| Versioned {
                record: record.clone(),
                revision: *revision,
            }))
    }

    async fn get_by_name(&self, tenant: &TenantId, name: &str) -> Result<Option<Versioned>, EntityStoreError> {
        let state = self.lock();
        let Some(id) = state.names.get(&(tenant.clone(), name.to_string())) else {
            return Ok(None);
        };
        Ok(state
            .entities
            .get(&(tenant.clone(), id.clone()))
            .map(|(record, revision)| Versioned {
                record: record.clone(),
                revision: *revision,
            }))
    }

    async fn list(&self, tenant: &TenantId) -> Result<Vec<Versioned>, EntityStoreError> {
        let mut out: Vec<Versioned> = self
            .lock()
            .entities
            .iter()
            .filter(|((t, _), _)| t == tenant)
            .map(|(_, (record, revision))| Versioned {
                record: record.clone(),
                revision: *revision,
            })
            .collect();
        out.sort_by(|a, b| (a.record.created_at, &a.record.entity_id).cmp(&(b.record.created_at, &b.record.entity_id)));
        Ok(out)
    }

    async fn list_all(&self) -> Result<Vec<Versioned>, EntityStoreError> {
        let mut out: Vec<Versioned> = self
            .lock()
            .entities
            .values()
            .map(|(record, revision)| Versioned {
                record: record.clone(),
                revision: *revision,
            })
            .collect();
        out.sort_by(|a, b| (a.record.created_at, &a.record.entity_id).cmp(&(b.record.created_at, &b.record.entity_id)));
        Ok(out)
    }

    async fn remove_dangling_name_claims(&self) -> Result<usize, EntityStoreError> {
        let mut state = self.lock();
        let dangling: Vec<(TenantId, String)> = state
            .names
            .iter()
            .filter(|((tenant, name), id)| {
                state
                    .entities
                    .get(&(tenant.clone(), (*id).clone()))
                    .is_none_or(|(record, _)| record.name.as_deref() != Some(name.as_str()))
            })
            .map(|(key, _)| key.clone())
            .collect();
        for key in &dangling {
            state.names.remove(key);
        }
        Ok(dangling.len())
    }

    async fn compare_and_swap(&self, record: &SandboxEntityRecord, revision: u64) -> Result<u64, EntityStoreError> {
        let mut state = self.lock();
        let key = (record.tenant.clone(), record.entity_id.clone());
        let Some((stored, stored_revision)) = state.entities.get_mut(&key) else {
            return Err(EntityStoreError::NotFound);
        };
        check_identity_unchanged(stored, record)?;
        if *stored_revision != revision {
            return Err(EntityStoreError::RevisionConflict);
        }
        *stored = record.clone();
        *stored_revision += 1;
        Ok(*stored_revision)
    }

    async fn delete(&self, tenant: &TenantId, id: &SandboxEntityId) -> Result<(), EntityStoreError> {
        let mut state = self.lock();
        let Some((record, _)) = state.entities.remove(&(tenant.clone(), id.clone())) else {
            return Err(EntityStoreError::NotFound);
        };
        if let Some(name) = record.name {
            let key = (tenant.clone(), name);
            if state.names.get(&key) == Some(id) {
                state.names.remove(&key);
            }
        }
        Ok(())
    }
}
