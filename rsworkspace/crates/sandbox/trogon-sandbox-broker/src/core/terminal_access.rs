use super::*;
use trogon_sandbox_contract::CapabilityNeed;

impl BrokerCore {
    /// Writes to the stdin of a previously started interactive command (see
    /// `RunCommandRequest::stdin`) via the owning worker. Routed like
    /// [`Self::handle_run_command`]; no worker answering the provider subject
    /// means the provider has no interactive stdin.
    pub async fn handle_write_stdin(&self, req: WriteStdinRequest) -> WriteStdinResponse {
        let record = match self.active_leases.get(req.lease_id()).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id(), "write_stdin denied: lease not found");
                return WriteStdinResponse::Denied(LeaseError::LeaseNotFound {
                    lease_id: req.lease_id().clone(),
                });
            }
            Err(e) => {
                warn!(error = %e, "write_stdin denied: active lease lookup failed");
                return WriteStdinResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id(), status = ?record.status, "write_stdin denied: lease is not running");
            return WriteStdinResponse::Denied(self.not_running_error(&record).await);
        }

        let lease_id = req.lease_id().clone();
        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return WriteStdinResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        self.touch_activity(&record, false).await;
        let subject = trogon_sandbox_protocol::subjects::provider_write_stdin(record.provider, &lease_id);
        let reply = match self
            .bus
            .request_with_timeout(&subject, body, PROVIDER_WRITE_STDIN_TIMEOUT)
            .await
        {
            Ok(r) => r,
            Err(e) if is_no_responders(&e) => return WriteStdinResponse::Unsupported,
            Err(e) if is_request_timeout(&e) => {
                return WriteStdinResponse::Backpressure { next_seq: req.seq() };
            }
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "write_stdin denied: worker dispatch failed");
                return WriteStdinResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match serde_json::from_slice::<WriteStdinResponse>(&reply) {
            Ok(resp) => resp,
            Err(e) => WriteStdinResponse::Denied(LeaseError::Backend {
                message: format!("write_stdin reply decode failed: {e}"),
            }),
        }
    }

    /// Tenant policy and shape checks for a lease's `native` payload. The
    /// returned error is what the lease is denied with.
    pub(super) async fn check_native_request(
        &self,
        tenant: &TenantId,
        native: &serde_json::Value,
        req: &LeaseRequest,
    ) -> Result<(), LeaseError> {
        match self.native_policy.allows(tenant).await {
            Ok(true) => {}
            Ok(false) => {
                warn!(tenant = %tenant, "lease request denied: vendor passthrough not permitted");
                return Err(LeaseError::NativePassthroughNotPermitted { tenant: tenant.clone() });
            }
            Err(e) => {
                return Err(LeaseError::Backend {
                    message: format!("native passthrough policy lookup failed: {e}"),
                });
            }
        }
        let first_preferred = req.provider_pref.first().copied().unwrap_or(ProviderKind::Mock);
        let Some(object) = native.as_object() else {
            return Err(LeaseError::InvalidNativeOptions {
                provider: first_preferred,
                reason: "native must be an object keyed by provider".to_string(),
            });
        };
        for key in object.keys() {
            if subjects::parse_provider_kind(key).is_err() {
                return Err(LeaseError::InvalidNativeOptions {
                    provider: first_preferred,
                    reason: format!("'{key}' is not a provider name"),
                });
            }
        }
        Ok(())
    }

    /// Returns the vendor's own handle on a running lease's sandbox via the
    /// owning worker. Denied unless the tenant policy allows passthrough, and
    /// `Unsupported` when the provider declares no passthrough support.
    pub async fn handle_native_handle(&self, req: NativeHandleRequest) -> NativeHandleResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "native_handle denied: lease not found");
                return NativeHandleResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "native_handle denied: active lease lookup failed");
                return NativeHandleResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match self.native_policy.allows(&record.tenant).await {
            Ok(true) => {}
            Ok(false) => {
                return NativeHandleResponse::Denied(LeaseError::NativePassthroughNotPermitted {
                    tenant: record.tenant,
                });
            }
            Err(e) => {
                return NativeHandleResponse::Denied(LeaseError::Backend {
                    message: format!("native passthrough policy lookup failed: {e}"),
                });
            }
        }

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id, status = ?record.status, "native_handle denied: lease is not running");
            return NativeHandleResponse::Denied(self.not_running_error(&record).await);
        }

        let requirements = CapabilityRequirements {
            needs_native_passthrough: CapabilityNeed::Required,
            ..Default::default()
        };
        if !self.provider_satisfies(record.provider, &requirements).await {
            return NativeHandleResponse::Unsupported;
        }

        let lease_id = req.lease_id.clone();
        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return NativeHandleResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        let subject = trogon_sandbox_protocol::subjects::provider_native_handle(record.provider, &lease_id);
        let reply = match self.bus.request(&subject, body).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "native_handle denied: worker dispatch failed");
                return NativeHandleResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match serde_json::from_slice::<NativeHandleResponse>(&reply) {
            Ok(resp) => resp,
            Err(e) => NativeHandleResponse::Denied(LeaseError::Backend {
                message: format!("native_handle reply decode failed: {e}"),
            }),
        }
    }

    /// Mints direct SSH access to a running lease's sandbox via the owning
    /// worker. SSH traffic then flows between the client and the provider,
    /// outside the broker, so the grant sits behind the same tenant policy as
    /// [`Self::handle_native_handle`]. A provider with no worker serving the
    /// verb is answered `Unsupported`.
    pub async fn handle_ssh_access(&self, req: SshAccessRequest) -> SshAccessResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                return SshAccessResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                return SshAccessResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match self.native_policy.allows(&record.tenant).await {
            Ok(true) => {}
            Ok(false) => {
                return SshAccessResponse::Denied(LeaseError::NativePassthroughNotPermitted { tenant: record.tenant });
            }
            Err(e) => {
                return SshAccessResponse::Denied(LeaseError::Backend {
                    message: format!("native passthrough policy lookup failed: {e}"),
                });
            }
        }

        if record.status != LeaseStatus::Running {
            return SshAccessResponse::Denied(self.not_running_error(&record).await);
        }

        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return SshAccessResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };
        let subject = trogon_sandbox_protocol::subjects::provider_ssh_access(record.provider, &req.lease_id);
        let reply = match self.bus.request(&subject, body).await {
            Ok(r) => r,
            Err(e) if is_no_responders(&e) => return SshAccessResponse::Unsupported,
            Err(e) => {
                warn!(error = %e, lease_id = %req.lease_id, "ssh_access denied: worker dispatch failed");
                return SshAccessResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match serde_json::from_slice::<SshAccessResponse>(&reply) {
            Ok(resp) => resp,
            Err(e) => SshAccessResponse::Denied(LeaseError::Backend {
                message: format!("ssh_access reply decode failed: {e}"),
            }),
        }
    }

    /// Opens, feeds, resizes or closes an interactive terminal on a running
    /// lease via the owning worker. Routed like [`Self::handle_run_command`],
    /// except that a provider whose profile lacks `supports_terminal` is
    /// answered `Unsupported` here, so no worker is asked. Every frame counts
    /// as lease activity for the idle timeout. Output the terminal produces on
    /// its own reaches the idle timer through the worker's activity signal
    /// (see [`Self::handle_worker_activity`]), since the broker does not read
    /// terminal output.
    pub async fn handle_terminal(&self, req: TerminalRequest) -> TerminalResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "terminal denied: lease not found");
                return TerminalResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "terminal denied: active lease lookup failed");
                return TerminalResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id, status = ?record.status, "terminal denied: lease is not running");
            return TerminalResponse::Denied(self.not_running_error(&record).await);
        }

        let requirements = CapabilityRequirements {
            needs_terminal: CapabilityNeed::Required,
            ..Default::default()
        };
        if !self.provider_satisfies(record.provider, &requirements).await {
            return TerminalResponse::Unsupported;
        }

        let lease_id = req.lease_id.clone();
        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return TerminalResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        let opens = matches!(req.op, TerminalOp::Open { .. });
        self.touch_activity(&record, opens).await;
        let subject = trogon_sandbox_protocol::subjects::provider_terminal(record.provider, &lease_id);
        let reply = match self
            .bus
            .request_with_timeout(&subject, body, PROVIDER_TERMINAL_TIMEOUT)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "terminal denied: worker dispatch failed");
                return TerminalResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match serde_json::from_slice::<TerminalResponse>(&reply) {
            Ok(resp) => resp,
            Err(e) => TerminalResponse::Denied(LeaseError::Backend {
                message: format!("terminal reply decode failed: {e}"),
            }),
        }
    }

    /// Resolves a port from an already-running lease's sandbox into a
    /// reachable endpoint via the owning worker. Routed like
    /// [`Self::handle_run_command`].
    pub async fn handle_get_sandbox_domain(&self, req: GetSandboxDomainRequest) -> GetSandboxDomainResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "get_sandbox_domain denied: lease not found");
                return GetSandboxDomainResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "get_sandbox_domain denied: active lease lookup failed");
                return GetSandboxDomainResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id, status = ?record.status, "get_sandbox_domain denied: lease is not running");
            return GetSandboxDomainResponse::Denied(self.not_running_error(&record).await);
        }

        let requirements = CapabilityRequirements {
            needs_port_preview: CapabilityNeed::Required,
            ..Default::default()
        };
        if !self.provider_satisfies(record.provider, &requirements).await {
            return GetSandboxDomainResponse::Unsupported;
        }

        let lease_id = req.lease_id.clone();
        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return GetSandboxDomainResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        let subject = trogon_sandbox_protocol::subjects::provider_get_domain(record.provider, &lease_id);
        let reply = match self.bus.request(&subject, body).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "get_sandbox_domain denied: worker dispatch failed");
                return GetSandboxDomainResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match serde_json::from_slice::<GetSandboxDomainResponse>(&reply) {
            Ok(resp) => resp,
            Err(e) => GetSandboxDomainResponse::Denied(LeaseError::Backend {
                message: format!("get_sandbox_domain reply decode failed: {e}"),
            }),
        }
    }
}
