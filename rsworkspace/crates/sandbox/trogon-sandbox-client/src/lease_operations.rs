use trogon_sandbox_contract::{
    CancelOperationRequest, CancelOperationResponse, CreatedLeaseCompensation, GetOperationRequest,
    GetOperationResponse, LeaseError, OperationCapabilities, OperationCapabilitiesRequest, OperationId,
    OperationRecord, TenantId,
};
use trogon_sandbox_protocol::subjects;

use crate::constants::SLOW_VERB_TIMEOUT;
use crate::sandbox_client::decode;
use crate::{ClientError, SandboxClient, TransportError};

impl SandboxClient {
    /// What the broker's durable operations can do for `tenant`.
    pub async fn operation_capabilities(&self, tenant: TenantId) -> Result<OperationCapabilities, ClientError> {
        let request = OperationCapabilitiesRequest { tenant };
        let payload = serde_json::to_vec(&request).map_err(TransportError::Encode)?;
        let bytes = self
            .call_bytes(subjects::OPERATION_CAPABILITIES_REQUEST, payload, SLOW_VERB_TIMEOUT)
            .await?;
        if let Ok(CancelOperationResponse::Denied(error)) = serde_json::from_slice(&bytes) {
            return Err(ClientError::Denied(error));
        }
        decode(&bytes)
    }

    /// Cancels the durable operation `operation_id`, keeping any lease it already created.
    pub async fn cancel_operation(
        &self,
        tenant: TenantId,
        operation_id: OperationId,
    ) -> Result<CancelOperationResponse, ClientError> {
        let request = CancelOperationRequest {
            tenant,
            operation_id,
            compensate_created: CreatedLeaseCompensation::Keep,
        };
        self.cancel_operation_request(request).await
    }

    /// Cancels a durable operation with every option of [`CancelOperationRequest`].
    pub async fn cancel_operation_request(
        &self,
        request: CancelOperationRequest,
    ) -> Result<CancelOperationResponse, ClientError> {
        match self
            .send(subjects::OPERATION_CANCEL_REQUEST, &request, SLOW_VERB_TIMEOUT)
            .await?
        {
            CancelOperationResponse::Denied(error) => Err(ClientError::Denied(error)),
            response => Ok(response),
        }
    }

    /// Reads one durable operation by the id the caller chose (or the id the broker returned).
    /// After a timeout on pause, resume or delete, this answers whether the request took effect.
    /// An unknown id, or one that belongs to another tenant, is `OperationNotFound`.
    pub async fn get_operation(
        &self,
        tenant: TenantId,
        operation_id: &OperationId,
    ) -> Result<OperationRecord, ClientError> {
        let request = GetOperationRequest {
            tenant,
            operation_id: operation_id.clone(),
        };
        match self
            .send(subjects::OPERATION_GET_REQUEST, &request, SLOW_VERB_TIMEOUT)
            .await?
        {
            GetOperationResponse::Found { operation } => Ok(*operation),
            GetOperationResponse::NotFound => Err(ClientError::Denied(LeaseError::OperationNotFound {
                operation_id: operation_id.clone(),
            })),
            GetOperationResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }
}
