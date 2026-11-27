//! Client verbs that read and delete persistent sandbox entities.
//!
//! Reads work whenever an entity store is installed (`shadow` or `on`).
//! Delete mutates, so it needs `on`; otherwise it would fight the legacy
//! stores that are still authoritative in `shadow`. Every lookup is keyed by
//! the authenticated tenant, so an id owned by another tenant is `NotFound`.

use trogon_sandbox_contract::SandboxEntityRecord;
use trogon_sandbox_contract::{
    DeleteEntityRequest, DeleteEntityResponse, EntityRef, GetEntityRequest, GetEntityResponse, ListEntitiesRequest,
    ListEntitiesResponse, SandboxEntityRow,
};

use super::*;
use crate::entity_shadow::EntityShadow;
use crate::entity_store::{EntityStoreError, Versioned};
use trogon_sandbox_contract::LeaseName;

impl BrokerCore {
    fn entity_reader(&self) -> Result<&Arc<EntityShadow>, LeaseError> {
        self.entity.as_ref().ok_or(LeaseError::EntityNotEnabled)
    }

    fn entity_writer(&self) -> Result<&Arc<EntityShadow>, LeaseError> {
        self.entity_reader().and_then(|shadow| match shadow.is_authoritative() {
            true => Ok(shadow),
            false => Err(LeaseError::EntityNotEnabled),
        })
    }

    fn entity_unavailable(op: &str, error: EntityStoreError) -> LeaseError {
        warn!(error = %error, op, "sandbox entity verb failed");
        LeaseError::EntityUnavailable {
            reason: format!("{op}: {error}"),
        }
    }

    async fn find_entity(
        shadow: &EntityShadow,
        tenant: &TenantId,
        entity: &EntityRef,
    ) -> Result<Option<Versioned>, EntityStoreError> {
        match entity {
            EntityRef::Id(id) => match shadow.store().get(tenant, id).await {
                Ok(found) => Ok(found),
                Err(EntityStoreError::NotFound) => Ok(None),
                Err(other) => Err(other),
            },
            EntityRef::Name(name) => shadow.store().get_by_name(tenant, name).await,
        }
    }

    pub async fn handle_get_entity(&self, req: GetEntityRequest) -> GetEntityResponse {
        let shadow = match self.entity_reader() {
            Ok(shadow) => shadow,
            Err(e) => return GetEntityResponse::Denied(e),
        };
        match Self::find_entity(shadow, &req.tenant, &req.entity).await {
            Ok(Some(found)) => GetEntityResponse::Found {
                row: SandboxEntityRow::from(&found.record),
            },
            Ok(None) => GetEntityResponse::NotFound,
            Err(e) => GetEntityResponse::Denied(Self::entity_unavailable("get", e)),
        }
    }

    pub async fn handle_list_entities(&self, req: ListEntitiesRequest) -> ListEntitiesResponse {
        let shadow = match self.entity_reader() {
            Ok(shadow) => shadow,
            Err(e) => return ListEntitiesResponse::Denied(e),
        };
        let mut records: Vec<SandboxEntityRecord> = match shadow.store().list(&req.tenant).await {
            Ok(listed) => listed.into_iter().map(|v| v.record).collect(),
            Err(e) => return ListEntitiesResponse::Denied(Self::entity_unavailable("list", e)),
        };
        records.sort_by(|a, b| a.entity_id.cmp(&b.entity_id));
        let page = usize::try_from(req.page_size()).unwrap_or(usize::MAX);
        let mut remaining: Vec<SandboxEntityRecord> = records
            .into_iter()
            .filter(|r| match &req.cursor {
                Some(cursor) => &r.entity_id > cursor,
                None => true,
            })
            .collect();
        let next_cursor = if remaining.len() > page {
            remaining.get(page.saturating_sub(1)).map(|r| r.entity_id.clone())
        } else {
            None
        };
        remaining.truncate(page);
        ListEntitiesResponse::Listed {
            rows: remaining.iter().map(SandboxEntityRow::from).collect(),
            next_cursor,
        }
    }

    /// Deletes an entity and everything that would let it come back: its
    /// running lease, its snapshot tag and its name. A named entity goes
    /// through the by-name delete so both verbs purge the same places.
    pub async fn handle_delete_entity(&self, req: DeleteEntityRequest) -> DeleteEntityResponse {
        let shadow = match self.entity_writer() {
            Ok(shadow) => shadow,
            Err(e) => return DeleteEntityResponse::Denied(e),
        };
        let found = match Self::find_entity(shadow, &req.tenant, &req.entity).await {
            Ok(Some(found)) => found.record,
            Ok(None) => return DeleteEntityResponse::NotFound,
            Err(e) => return DeleteEntityResponse::Denied(Self::entity_unavailable("delete", e)),
        };

        if let Err(e) = self
            .entity_intend_delete(&req.tenant, &EntityRef::Id(found.entity_id.clone()))
            .await
        {
            return DeleteEntityResponse::Denied(e);
        }

        if let Some(name) = &found.name {
            let name = match LeaseName::new(name.as_str()) {
                Ok(name) => name,
                Err(e) => {
                    return DeleteEntityResponse::Denied(LeaseError::InvalidSpec { reason: e.to_string() });
                }
            };
            return match self
                .handle_delete_named_sandbox(DeleteNamedSandboxRequest {
                    tenant: req.tenant.clone(),
                    name,
                })
                .await
            {
                DeleteNamedSandboxResponse::Deleted => DeleteEntityResponse::Deleted,
                DeleteNamedSandboxResponse::NotFound => DeleteEntityResponse::NotFound,
                DeleteNamedSandboxResponse::Denied(e) => DeleteEntityResponse::Denied(e),
            };
        }

        if let Some(lease_id) = &found.current_lease
            && let Err(e) = self.cancel_lease(lease_id.clone(), CancelIntent::Purge).await
        {
            warn!(error = %e, entity_id = %found.entity_id, "delete entity denied: cancel failed");
            return DeleteEntityResponse::Denied(LeaseError::Backend { message: e.to_string() });
        }
        match self
            .handle_snapshot_tag_delete(DeleteSnapshotTagRequest {
                tenant: req.tenant.clone(),
                name: found.persistence_tag.name.clone(),
            })
            .await
        {
            DeleteSnapshotTagResponse::Applied | DeleteSnapshotTagResponse::NotFound => {}
            DeleteSnapshotTagResponse::Denied(e) => return DeleteEntityResponse::Denied(e),
        }
        match shadow.delete_entity(&req.tenant, &found.entity_id).await {
            Ok(true) => DeleteEntityResponse::Deleted,
            Ok(false) => DeleteEntityResponse::NotFound,
            Err(e) => DeleteEntityResponse::Denied(Self::entity_unavailable("delete", e)),
        }
    }
}
