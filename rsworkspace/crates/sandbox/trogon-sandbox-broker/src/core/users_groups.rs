use super::*;
use trogon_sandbox_contract::CapabilityNeed;

impl BrokerCore {
    /// Creates a per-sandbox Linux user inside an already-running lease's
    /// sandbox via the owning worker. Routed like [`Self::handle_run_command`].
    pub async fn handle_create_user(&self, req: CreateSandboxUserRequest) -> CreateSandboxUserResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "create_user denied: lease not found");
                return CreateSandboxUserResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "create_user denied: active lease lookup failed");
                return CreateSandboxUserResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id, status = ?record.status, "create_user denied: lease is not running");
            return CreateSandboxUserResponse::Denied(self.not_running_error(&record).await);
        }

        let lease_id = req.lease_id.clone();
        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return CreateSandboxUserResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        let subject = trogon_sandbox_protocol::subjects::provider_create_user(record.provider, &lease_id);
        let reply = match self.bus.request(&subject, body).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "create_user denied: worker dispatch failed");
                return CreateSandboxUserResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match serde_json::from_slice::<CreateSandboxUserResponse>(&reply) {
            Ok(resp) => resp,
            Err(e) => CreateSandboxUserResponse::Denied(LeaseError::Backend {
                message: format!("create_user reply decode failed: {e}"),
            }),
        }
    }

    /// Creates a per-sandbox Linux group inside an already-running lease's
    /// sandbox via the owning worker. Routed like [`Self::handle_run_command`].
    pub async fn handle_create_group(&self, req: CreateSandboxGroupRequest) -> CreateSandboxGroupResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "create_group denied: lease not found");
                return CreateSandboxGroupResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "create_group denied: active lease lookup failed");
                return CreateSandboxGroupResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id, status = ?record.status, "create_group denied: lease is not running");
            return CreateSandboxGroupResponse::Denied(self.not_running_error(&record).await);
        }

        let requirements = CapabilityRequirements {
            needs_sandbox_groups: CapabilityNeed::Required,
            ..Default::default()
        };
        if !self.provider_satisfies(record.provider, &requirements).await {
            warn!(lease_id = %req.lease_id, provider = ?record.provider, "create_group denied: provider has no sandbox groups");
            return CreateSandboxGroupResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { requirements });
        }

        let lease_id = req.lease_id.clone();
        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return CreateSandboxGroupResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        let subject = trogon_sandbox_protocol::subjects::provider_create_group(record.provider, &lease_id);
        let reply = match self.bus.request(&subject, body).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "create_group denied: worker dispatch failed");
                return CreateSandboxGroupResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match serde_json::from_slice::<CreateSandboxGroupResponse>(&reply) {
            Ok(resp) => resp,
            Err(e) => CreateSandboxGroupResponse::Denied(LeaseError::Backend {
                message: format!("create_group reply decode failed: {e}"),
            }),
        }
    }

    /// Adds or removes a sandbox user's group membership inside an
    /// already-running lease's sandbox via the owning worker. Routed like
    /// [`Self::handle_run_command`].
    pub async fn handle_modify_group_membership(
        &self,
        req: ModifyGroupMembershipRequest,
    ) -> ModifyGroupMembershipResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "modify_group_membership denied: lease not found");
                return ModifyGroupMembershipResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "modify_group_membership denied: active lease lookup failed");
                return ModifyGroupMembershipResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id, status = ?record.status, "modify_group_membership denied: lease is not running");
            return ModifyGroupMembershipResponse::Denied(self.not_running_error(&record).await);
        }

        let requirements = CapabilityRequirements {
            needs_sandbox_groups: CapabilityNeed::Required,
            ..Default::default()
        };
        if !self.provider_satisfies(record.provider, &requirements).await {
            warn!(lease_id = %req.lease_id, provider = ?record.provider, "modify_group_membership denied: provider has no sandbox groups");
            return ModifyGroupMembershipResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { requirements });
        }

        let lease_id = req.lease_id.clone();
        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return ModifyGroupMembershipResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        let subject = trogon_sandbox_protocol::subjects::provider_modify_group_membership(record.provider, &lease_id);
        let reply = match self.bus.request(&subject, body).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "modify_group_membership denied: worker dispatch failed");
                return ModifyGroupMembershipResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match serde_json::from_slice::<ModifyGroupMembershipResponse>(&reply) {
            Ok(resp) => resp,
            Err(e) => ModifyGroupMembershipResponse::Denied(LeaseError::Backend {
                message: format!("modify_group_membership reply decode failed: {e}"),
            }),
        }
    }
}
