use super::*;

impl BrokerCore {
    /// Polls the status of a previously started detached command (see
    /// `RunCommandRequest::detached`) inside an already-running lease's
    /// sandbox. Routed like [`Self::handle_run_command`].
    pub async fn handle_poll_command(&self, req: PollCommandRequest) -> PollCommandResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "poll_command denied: lease not found");
                return PollCommandResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "poll_command denied: active lease lookup failed");
                return PollCommandResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id, status = ?record.status, "poll_command denied: lease is not running");
            return PollCommandResponse::Denied(LeaseError::LeaseNotRunning { lease_id: req.lease_id });
        }

        let lease_id = req.lease_id.clone();
        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return PollCommandResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        self.touch_activity(&record, false).await;
        let subject = trogon_sandbox_protocol::subjects::provider_poll_command(record.provider, &lease_id);
        let reply = match self.bus.request(&subject, body).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "poll_command denied: worker dispatch failed");
                return PollCommandResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match serde_json::from_slice::<PollCommandResponse>(&reply) {
            Ok(resp) => {
                if let PollCommandResponse::Completed(out) = &resp {
                    self.count_usage(
                        &record,
                        LeaseUsageCounters {
                            command_output_bytes: (out.stdout.len() + out.stderr.len()) as u64,
                            ..Default::default()
                        },
                    )
                    .await;
                }
                resp
            }
            Err(e) => PollCommandResponse::Denied(LeaseError::Backend {
                message: format!("poll_command reply decode failed: {e}"),
            }),
        }
    }

    /// Sends a Unix signal to a previously started command inside an
    /// already-running lease's sandbox via the owning worker. Routed like
    /// [`Self::handle_run_command`].
    pub async fn handle_kill_command(&self, req: KillCommandRequest) -> KillCommandResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "kill_command denied: lease not found");
                return KillCommandResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "kill_command denied: active lease lookup failed");
                return KillCommandResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id, status = ?record.status, "kill_command denied: lease is not running");
            return KillCommandResponse::Denied(LeaseError::LeaseNotRunning { lease_id: req.lease_id });
        }

        let lease_id = req.lease_id.clone();
        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return KillCommandResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        self.touch_activity(&record, false).await;
        let subject = trogon_sandbox_protocol::subjects::provider_kill_command(record.provider, &lease_id);
        let reply = match self.bus.request(&subject, body).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "kill_command denied: worker dispatch failed");
                return KillCommandResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match serde_json::from_slice::<KillCommandResponse>(&reply) {
            Ok(resp) => resp,
            Err(e) => KillCommandResponse::Denied(LeaseError::Backend {
                message: format!("kill_command reply decode failed: {e}"),
            }),
        }
    }
}
