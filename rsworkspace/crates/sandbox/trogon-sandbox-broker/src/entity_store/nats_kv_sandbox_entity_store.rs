use super::*;

pub struct NatsKvSandboxEntityStore {
    entities: kv::Store,
    names: kv::Store,
}

impl NatsKvSandboxEntityStore {
    pub async fn new(jetstream: jetstream::Context) -> Result<Self, EntityStoreError> {
        let entities = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: ENTITY_KV_BUCKET.to_string(),
                history: ENTITY_KV_HISTORY,
                ..Default::default()
            })
            .await
            .map_err(EntityStoreError::backend)?;
        let names = jetstream
            .create_or_update_key_value(kv::Config {
                bucket: ENTITY_NAME_INDEX_KV_BUCKET.to_string(),
                history: ENTITY_KV_HISTORY,
                ..Default::default()
            })
            .await
            .map_err(EntityStoreError::backend)?;
        Ok(Self { entities, names })
    }

    async fn read_entity(&self, key: &str) -> Result<Option<Versioned>, EntityStoreError> {
        match self.entities.entry(key).await {
            Ok(None) => Ok(None),
            Ok(Some(entry)) if matches!(entry.operation, Operation::Delete | Operation::Purge) => Ok(None),
            Ok(Some(entry)) => {
                let record: SandboxEntityRecord =
                    serde_json::from_slice(&entry.value).map_err(EntityStoreError::DecodeEntity)?;
                Ok(Some(Versioned {
                    record,
                    revision: entry.revision,
                }))
            }
            Err(e) => Err(EntityStoreError::backend(e)),
        }
    }

    /// Reads and re-checks that the stored record belongs to `tenant` and `id`.
    async fn read_checked(
        &self,
        tenant: &TenantId,
        id: &SandboxEntityId,
    ) -> Result<Option<Versioned>, EntityStoreError> {
        Ok(self
            .read_entity(&entity_kv_key(tenant, id))
            .await?
            .filter(|v| v.record.tenant == *tenant && v.record.entity_id == *id))
    }

    async fn read_claim(&self, key: &str) -> Result<Option<(SandboxEntityId, u64)>, EntityStoreError> {
        match self.names.entry(key).await {
            Ok(None) => Ok(None),
            Ok(Some(entry)) if matches!(entry.operation, Operation::Delete | Operation::Purge) => Ok(None),
            Ok(Some(entry)) => {
                let id = std::str::from_utf8(&entry.value)
                    .ok()
                    .and_then(|s| SandboxEntityId::new(s).ok())
                    .ok_or(EntityStoreError::CorruptNameClaim)?;
                Ok(Some((id, entry.revision)))
            }
            Err(e) => Err(EntityStoreError::backend(e)),
        }
    }

    /// Claim `name` for an entity that already exists. A claim whose target is
    /// gone (a crash between delete steps) is dangling and is taken over with a
    /// CAS; a live target means the name is taken.
    async fn claim_name(&self, tenant: &TenantId, name: &str, id: &SandboxEntityId) -> Result<(), EntityStoreError> {
        let key = entity_name_kv_key(tenant, name);
        let value = Bytes::from(id.to_string());
        for _ in 0..MAX_CAS_RETRIES {
            match self.names.create(&key, value.clone()).await {
                Ok(_) => return Ok(()),
                Err(e) if e.kind() == kv::CreateErrorKind::AlreadyExists => {}
                Err(e) => return Err(EntityStoreError::backend(e)),
            }
            let Some((holder, revision)) = self.read_claim(&key).await? else {
                continue;
            };
            let live = self
                .read_checked(tenant, &holder)
                .await?
                .is_some_and(|v| v.record.name.as_deref() == Some(name));
            if live {
                return Err(EntityStoreError::NameTaken);
            }
            match self.names.update(&key, value.clone(), revision).await {
                Ok(_) => return Ok(()),
                Err(e) if e.kind() == kv::UpdateErrorKind::WrongLastRevision => continue,
                Err(e) => return Err(EntityStoreError::backend(e)),
            }
        }
        Err(EntityStoreError::NameClaimRetriesExhausted)
    }

    async fn release_claim(&self, tenant: &TenantId, name: &str, id: &SandboxEntityId) -> Result<(), EntityStoreError> {
        let key = entity_name_kv_key(tenant, name);
        for _ in 0..MAX_CAS_RETRIES {
            let Some((holder, revision)) = self.read_claim(&key).await? else {
                return Ok(());
            };
            if holder != *id {
                return Ok(());
            }
            match self.names.delete_expect_revision(&key, Some(revision)).await {
                Ok(()) => return Ok(()),
                Err(e) if e.kind() == kv::DeleteErrorKind::WrongLastRevision => continue,
                Err(e) => return Err(EntityStoreError::backend(e)),
            }
        }
        Err(EntityStoreError::NameReleaseRetriesExhausted)
    }
}

#[async_trait]
impl SandboxEntityStore for NatsKvSandboxEntityStore {
    async fn create(&self, new: NewSandboxEntity) -> Result<Versioned, EntityStoreError> {
        if let Some(name) = &new.name {
            validate_name(name)?;
        }
        let record = build_record(new, new_entity_id()?)?;
        let key = entity_kv_key(&record.tenant, &record.entity_id);
        let payload = serde_json::to_vec(&record).map_err(EntityStoreError::backend)?;
        // Entity first, claim second: a claim then never points at a record
        // that was not yet written, so a missing target always means dangling.
        let revision = self
            .entities
            .create(&key, Bytes::from(payload))
            .await
            .map_err(EntityStoreError::backend)?;
        if let Some(name) = &record.name
            && let Err(error) = self.claim_name(&record.tenant, name, &record.entity_id).await
        {
            let _ = self.entities.delete_expect_revision(&key, Some(revision)).await;
            return Err(error);
        }
        Ok(Versioned { record, revision })
    }

    async fn get(&self, tenant: &TenantId, id: &SandboxEntityId) -> Result<Option<Versioned>, EntityStoreError> {
        self.read_checked(tenant, id).await
    }

    async fn get_by_name(&self, tenant: &TenantId, name: &str) -> Result<Option<Versioned>, EntityStoreError> {
        let Some((id, _)) = self.read_claim(&entity_name_kv_key(tenant, name)).await? else {
            return Ok(None);
        };
        Ok(self
            .read_checked(tenant, &id)
            .await?
            .filter(|v| v.record.name.as_deref() == Some(name)))
    }

    async fn list(&self, tenant: &TenantId) -> Result<Vec<Versioned>, EntityStoreError> {
        let prefix = entity_tenant_prefix(tenant);
        let mut keys = self.entities.keys().await.map_err(EntityStoreError::backend)?;
        let mut out = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(EntityStoreError::backend)?;
            if !key.starts_with(&prefix) {
                continue;
            }
            if let Some(v) = self.read_entity(&key).await?
                && v.record.tenant == *tenant
                && key == entity_kv_key(tenant, &v.record.entity_id)
            {
                out.push(v);
            }
        }
        out.sort_by(|a, b| (a.record.created_at, &a.record.entity_id).cmp(&(b.record.created_at, &b.record.entity_id)));
        Ok(out)
    }

    async fn list_all(&self) -> Result<Vec<Versioned>, EntityStoreError> {
        let mut keys = self.entities.keys().await.map_err(EntityStoreError::backend)?;
        let mut out = Vec::new();
        while let Some(key) = keys.next().await {
            let key = key.map_err(EntityStoreError::backend)?;
            if !key.starts_with("e.") {
                continue;
            }
            if let Some(v) = self.read_entity(&key).await?
                && key == entity_kv_key(&v.record.tenant, &v.record.entity_id)
            {
                out.push(v);
            }
        }
        out.sort_by(|a, b| (a.record.created_at, &a.record.entity_id).cmp(&(b.record.created_at, &b.record.entity_id)));
        Ok(out)
    }

    async fn remove_dangling_name_claims(&self) -> Result<usize, EntityStoreError> {
        let mut keys = self.names.keys().await.map_err(EntityStoreError::backend)?;
        let mut all = Vec::new();
        while let Some(key) = keys.next().await {
            all.push(key.map_err(EntityStoreError::backend)?);
        }
        let mut removed = 0;
        for key in all {
            let mut parts = key.splitn(3, '.');
            let (Some("n"), Some(tenant_hex), Some(name_hex)) = (parts.next(), parts.next(), parts.next()) else {
                continue;
            };
            let (Some(tenant), Some(name)) = (decode_hex_token(tenant_hex), decode_hex_token(name_hex)) else {
                continue;
            };
            let Ok(tenant) = TenantId::new(tenant) else {
                continue;
            };
            let Some((holder, revision)) = self.read_claim(&key).await? else {
                continue;
            };
            let live = self
                .read_checked(&tenant, &holder)
                .await?
                .is_some_and(|v| v.record.name.as_deref() == Some(name.as_str()));
            if live {
                continue;
            }
            match self.names.delete_expect_revision(&key, Some(revision)).await {
                Ok(()) => removed += 1,
                Err(e) if e.kind() == kv::DeleteErrorKind::WrongLastRevision => {}
                Err(e) => return Err(EntityStoreError::backend(e)),
            }
        }
        Ok(removed)
    }

    async fn compare_and_swap(&self, record: &SandboxEntityRecord, revision: u64) -> Result<u64, EntityStoreError> {
        let key = entity_kv_key(&record.tenant, &record.entity_id);
        let Some(stored) = self.read_entity(&key).await? else {
            return Err(EntityStoreError::NotFound);
        };
        check_identity_unchanged(&stored.record, record)?;
        let payload = serde_json::to_vec(record).map_err(EntityStoreError::backend)?;
        match self.entities.update(&key, Bytes::from(payload), revision).await {
            Ok(revision) => Ok(revision),
            Err(e) if e.kind() == kv::UpdateErrorKind::WrongLastRevision => Err(EntityStoreError::RevisionConflict),
            Err(e) => Err(EntityStoreError::backend(e)),
        }
    }

    async fn delete(&self, tenant: &TenantId, id: &SandboxEntityId) -> Result<(), EntityStoreError> {
        let key = entity_kv_key(tenant, id);
        for _ in 0..MAX_CAS_RETRIES {
            let Some(found) = self.read_checked(tenant, id).await? else {
                return Err(EntityStoreError::NotFound);
            };
            match self.entities.delete_expect_revision(&key, Some(found.revision)).await {
                Ok(()) => {
                    if let Some(name) = &found.record.name {
                        self.release_claim(tenant, name, id).await?;
                    }
                    return Ok(());
                }
                Err(e) if e.kind() == kv::DeleteErrorKind::WrongLastRevision => continue,
                Err(e) => return Err(EntityStoreError::backend(e)),
            }
        }
        Err(EntityStoreError::DeleteRetriesExhausted)
    }
}
