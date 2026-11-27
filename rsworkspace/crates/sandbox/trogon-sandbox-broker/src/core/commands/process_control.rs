use super::*;
use trogon_sandbox_contract::{CapabilityNeed, CommandExitReport, LogsContinuation, OutputTruncation};

impl BrokerCore {
    /// Stops a detached command gracefully (SIGTERM, then SIGKILL after the
    /// grace period) via the owning worker. A provider that does not declare
    /// graceful stop, or has no worker answering the verb, replies
    /// [`StopCommandResponse::Unsupported`].
    pub async fn handle_stop_command(&self, mut req: StopCommandRequest) -> StopCommandResponse {
        let provider = match self.running_lease_provider(&req.lease_id).await {
            Ok(p) => p,
            Err(e) => {
                warn!(lease_id = %req.lease_id, error = %e, "stop_command denied");
                return StopCommandResponse::Denied(e);
            }
        };
        let requirements = CapabilityRequirements {
            needs_graceful_stop: CapabilityNeed::Required,
            ..Default::default()
        };
        if !self.provider_satisfies(provider, &requirements).await {
            return StopCommandResponse::Unsupported;
        }
        req.grace_seconds = req.grace_seconds.min(StopCommandRequest::MAX_GRACE_SECONDS);
        let timeout = Duration::from_secs(u64::from(req.grace_seconds) + 10);
        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return StopCommandResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };
        let subject = trogon_sandbox_protocol::subjects::provider_stop_command(provider, &req.lease_id);
        let reply = match self.bus.request_with_timeout(&subject, body, timeout).await {
            Ok(r) => r,
            Err(e) if is_no_responders(&e) => return StopCommandResponse::Unsupported,
            Err(e) => {
                warn!(error = %e, lease_id = %req.lease_id, "stop_command denied: worker dispatch failed");
                return StopCommandResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };
        match serde_json::from_slice::<StopCommandResponse>(&reply) {
            Ok(resp) => resp,
            Err(e) => StopCommandResponse::Denied(LeaseError::Backend {
                message: format!("stop_command reply decode failed: {e}"),
            }),
        }
    }

    /// Runs a detached command again from the request it started with, via
    /// the owning worker. Unsupported like [`Self::handle_stop_command`].
    pub async fn handle_restart_command(&self, mut req: RestartCommandRequest) -> RestartCommandResponse {
        let provider = match self.running_lease_provider(&req.lease_id).await {
            Ok(p) => p,
            Err(e) => {
                warn!(lease_id = %req.lease_id, error = %e, "restart_command denied");
                return RestartCommandResponse::Denied(e);
            }
        };
        let requirements = CapabilityRequirements {
            needs_command_restart: CapabilityNeed::Required,
            ..Default::default()
        };
        if !self.provider_satisfies(provider, &requirements).await {
            return RestartCommandResponse::Unsupported;
        }
        req.grace_seconds = req.grace_seconds.min(StopCommandRequest::MAX_GRACE_SECONDS);
        let timeout = Duration::from_secs(u64::from(req.grace_seconds) + 10);
        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return RestartCommandResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };
        let subject = trogon_sandbox_protocol::subjects::provider_restart_command(provider, &req.lease_id);
        let reply = match self.bus.request_with_timeout(&subject, body, timeout).await {
            Ok(r) => r,
            Err(e) if is_no_responders(&e) => return RestartCommandResponse::Unsupported,
            Err(e) => {
                warn!(error = %e, lease_id = %req.lease_id, "restart_command denied: worker dispatch failed");
                return RestartCommandResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };
        match serde_json::from_slice::<RestartCommandResponse>(&reply) {
            Ok(resp) => resp,
            Err(e) => RestartCommandResponse::Denied(LeaseError::Backend {
                message: format!("restart_command reply decode failed: {e}"),
            }),
        }
    }

    /// Reads a page of a streamed command's retained output. The lease must
    /// still be active so the transport layer can authenticate the caller
    /// against its tenant; a deployment without a log store, or a lease on a
    /// provider whose profile lacks `supports_output_streaming`, replies
    /// [`ReadLogsResponse::Unsupported`].
    pub async fn handle_read_logs(&self, req: ReadLogsRequest) -> ReadLogsResponse {
        let Some(store) = &self.command_logs else {
            return ReadLogsResponse::Unsupported;
        };
        match self.active_leases.get(&req.lease_id).await {
            Ok(Some(record)) => {
                // A provider that does not stream output never publishes a chunk, so an
                // empty page would read as a command with no output.
                let requirements = CapabilityRequirements {
                    needs_output_streaming: CapabilityNeed::Required,
                    ..Default::default()
                };
                if !self.provider_satisfies(record.provider, &requirements).await {
                    return ReadLogsResponse::Unsupported;
                }
            }
            Ok(None) => {
                return ReadLogsResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                return ReadLogsResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        }
        let read = || {
            store.read(
                &req.lease_id,
                &req.execution_id,
                req.after_seq,
                req.max_bytes
                    .unwrap_or(ReadLogsRequest::DEFAULT_BYTES)
                    .clamp(1, ReadLogsRequest::MAX_BYTES),
            )
        };
        let mut result = read().await;
        let wait_ms = req.wait_ms.unwrap_or(0).min(ReadLogsRequest::MAX_WAIT_MS);
        if wait_ms > 0 && matches!(&result, Ok(page) if page.chunks.is_empty() && !page.has_more && !page.exited) {
            // Nothing past the cursor yet: park until a chunk lands or the
            // wait runs out, then read once more.
            result = match store
                .wait_for_chunks(
                    &req.lease_id,
                    &req.execution_id,
                    req.after_seq,
                    Duration::from_millis(wait_ms),
                )
                .await
            {
                Ok(()) => read().await,
                Err(e) => Err(e),
            };
        }
        match result {
            Ok(page) => ReadLogsResponse::Logs {
                chunks: page.chunks,
                next_seq: page.next_seq,
                has_more: LogsContinuation::from(page.has_more),
                truncated: OutputTruncation::from(page.truncated),
                exited: CommandExitReport::from(page.exited),
            },
            Err(crate::command_logs::CommandLogError::InvalidExecution) => {
                ReadLogsResponse::Denied(LeaseError::Backend {
                    message: "execution id is not valid".to_string(),
                })
            }
            Err(e) => {
                warn!(error = %e, lease_id = %req.lease_id, "read_logs denied: log store failed");
                ReadLogsResponse::Denied(LeaseError::Backend { message: e.to_string() })
            }
        }
    }
}
