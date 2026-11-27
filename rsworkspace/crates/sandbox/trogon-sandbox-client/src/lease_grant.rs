use std::time::{Duration, SystemTime};

use bytes::Bytes;
use trogon_sandbox_contract::{
    ExtendLeaseTimeoutRequest, ExtendLeaseTimeoutResponse, LeaseGranted, LeaseId, LeaseRequest, LeaseResponse,
    NetworkPolicy, NetworkPolicyUpdateRequest, NetworkPolicyUpdateResponse, OperationId, SandboxTags,
    UpdateLeaseTagsRequest, UpdateLeaseTagsResponse,
};
use trogon_sandbox_protocol::subjects;

use crate::constants::QUICK_VERB_TIMEOUT;
use crate::{ClientError, SandboxClient};

impl SandboxClient {
    pub async fn lease(&self, request: &LeaseRequest, timeout: Duration) -> Result<LeaseGranted, ClientError> {
        match self
            .send::<_, LeaseResponse>(subjects::LEASE_REQUEST, request, timeout)
            .await?
        {
            LeaseResponse::Granted(granted) => Ok(granted),
            LeaseResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }

    /// Creates (or restores, when the image is a snapshot) as a durable
    /// operation. The same operation id again replays the first grant instead
    /// of creating a second sandbox, and a lost reply does not lose the lease.
    /// `None` behaves exactly like [`Self::lease`].
    pub async fn lease_with_operation(
        &self,
        request: &LeaseRequest,
        operation_id: Option<OperationId>,
        timeout: Duration,
    ) -> Result<LeaseGranted, ClientError> {
        match operation_id {
            None => self.lease(request, timeout).await,
            Some(operation_id) => {
                let mut request = request.clone();
                request.operation_id = Some(operation_id);
                self.lease(&request, timeout).await
            }
        }
    }

    /// Cancels `lease_id`, preserving resumable state. The cancel is fire and forget: a lost message is not an
    /// error.
    pub async fn cancel(&self, lease_id: &LeaseId) -> Result<(), ClientError> {
        let subject = subjects::lease_cancel(lease_id);
        let _ = self
            .bus
            .request(&subject, Bytes::new(), QUICK_VERB_TIMEOUT, self.build_auth_headers())
            .await;
        Ok(())
    }

    /// Fully replaces `lease_id`'s tag set with `tags` (not a merge).
    pub async fn update_tags(&self, lease_id: &LeaseId, tags: SandboxTags) -> Result<(), ClientError> {
        let request = UpdateLeaseTagsRequest {
            lease_id: lease_id.clone(),
            tags,
        };
        match self
            .send(&subjects::lease_tags_update(lease_id), &request, QUICK_VERB_TIMEOUT)
            .await?
        {
            UpdateLeaseTagsResponse::Applied => Ok(()),
            UpdateLeaseTagsResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }

    /// Replaces `lease_id`'s egress `network_policy` in place, without
    /// restarting or re-provisioning the underlying sandbox.
    pub async fn update_network_policy(
        &self,
        lease_id: &LeaseId,
        network_policy: NetworkPolicy,
    ) -> Result<(), ClientError> {
        let request = NetworkPolicyUpdateRequest {
            lease_id: lease_id.clone(),
            network_policy,
        };
        match self
            .send(
                &subjects::lease_network_policy_update(lease_id),
                &request,
                QUICK_VERB_TIMEOUT,
            )
            .await?
        {
            NetworkPolicyUpdateResponse::Applied => Ok(()),
            NetworkPolicyUpdateResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }

    /// Extends `lease_id`'s wall-clock deadline by `additional_seconds` without restarting it.
    pub async fn extend_timeout(&self, lease_id: &LeaseId, additional_seconds: u32) -> Result<SystemTime, ClientError> {
        let request = ExtendLeaseTimeoutRequest {
            lease_id: lease_id.clone(),
            additional_seconds,
        };
        match self
            .send(&subjects::lease_extend_timeout(lease_id), &request, QUICK_VERB_TIMEOUT)
            .await?
        {
            ExtendLeaseTimeoutResponse::Extended { new_deadline } => Ok(new_deadline),
            ExtendLeaseTimeoutResponse::Denied(error) => Err(ClientError::Denied(error)),
        }
    }
}
