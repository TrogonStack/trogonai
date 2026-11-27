use trogon_sandbox_contract::{
    DeleteEntityRequest, DeleteEntityResponse, EntityRef, GetEntityRequest, GetEntityResponse, ListEntitiesRequest,
    ListEntitiesResponse, TenantId,
};
use trogon_sandbox_protocol::subjects;

use crate::{ClientError, SandboxClient};

impl SandboxClient {
    /// Reads one persistent sandbox entity, including a stopped one. Needs a broker with entity tracking enabled;
    /// otherwise the denial is `LeaseError::EntityNotEnabled`.
    pub async fn get_entity(&self, tenant: TenantId, entity: EntityRef) -> Result<GetEntityResponse, ClientError> {
        let request = GetEntityRequest { tenant, entity };
        let response: GetEntityResponse = self.send_default(subjects::ENTITY_GET_REQUEST, &request).await?;
        match response {
            GetEntityResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Lists a tenant's entities ordered by id. Pass the previous page's `next_cursor` as `cursor` to continue.
    pub async fn list_entities(&self, request: ListEntitiesRequest) -> Result<ListEntitiesResponse, ClientError> {
        let response: ListEntitiesResponse = self.send_default(subjects::ENTITY_LIST_REQUEST, &request).await?;
        match response {
            ListEntitiesResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }

    /// Permanently deletes an entity with its running lease, snapshots and name. Needs a broker with entity tracking
    /// fully enabled.
    pub async fn delete_entity(
        &self,
        tenant: TenantId,
        entity: EntityRef,
    ) -> Result<DeleteEntityResponse, ClientError> {
        let request = DeleteEntityRequest { tenant, entity };
        let response: DeleteEntityResponse = self.send_default(subjects::ENTITY_DELETE_REQUEST, &request).await?;
        match response {
            DeleteEntityResponse::Denied(error) => Err(ClientError::Denied(error)),
            other => Ok(other),
        }
    }
}
