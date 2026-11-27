use super::*;
use trogon_sandbox_contract::{CapabilityNeed, CommandExecution, LaunchCancellation, StreamedOutput};

impl BrokerCore {
    /// Lists worker-owned executions independently of named-launch support.
    /// The owning worker reports unsupported inventory, including when it has
    /// no responder for this read operation.
    pub async fn handle_list_processes(&self, req: ListProcessesRequest) -> ListProcessesResponse {
        let provider = match self.running_lease_provider(&req.lease_id).await {
            Ok(p) => p,
            Err(e) => {
                warn!(lease_id = %req.lease_id, error = %e, "list_processes denied");
                return ListProcessesResponse::Denied(e);
            }
        };
        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return ListProcessesResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };
        let subject = trogon_sandbox_protocol::subjects::provider_list_processes(provider, &req.lease_id);
        let reply = match self.bus.request(&subject, body).await {
            Ok(r) => r,
            Err(e) if is_no_responders(&e) => return ListProcessesResponse::Unsupported,
            Err(e) => {
                warn!(error = %e, lease_id = %req.lease_id, "list_processes denied: worker dispatch failed");
                return ListProcessesResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };
        match serde_json::from_slice::<ListProcessesResponse>(&reply) {
            Ok(resp) => resp,
            Err(e) => ListProcessesResponse::Denied(LeaseError::Backend {
                message: format!("list_processes reply decode failed: {e}"),
            }),
        }
    }

    /// Waits for a process's readiness probe via the owning worker, bounded by
    /// the request's timeout. Unsupported like [`Self::handle_list_processes`].
    pub async fn handle_wait_ready(&self, mut req: WaitReadyRequest) -> WaitReadyResponse {
        let provider = match self.running_lease_provider(&req.lease_id).await {
            Ok(p) => p,
            Err(e) => {
                warn!(lease_id = %req.lease_id, error = %e, "wait_ready denied");
                return WaitReadyResponse::Denied(e);
            }
        };
        let requirements = CapabilityRequirements {
            needs_process_readiness: CapabilityNeed::Required,
            ..Default::default()
        };
        if !self.provider_satisfies(provider, &requirements).await {
            return WaitReadyResponse::Unsupported;
        }
        req.timeout_seconds = req.timeout_seconds.min(WaitReadyRequest::MAX_TIMEOUT_SECONDS);
        let timeout = Duration::from_secs(u64::from(req.timeout_seconds) + 10);
        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return WaitReadyResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };
        let subject = trogon_sandbox_protocol::subjects::provider_wait_ready(provider, &req.lease_id);
        let reply = match self.bus.request_with_timeout(&subject, body, timeout).await {
            Ok(r) => r,
            Err(e) if is_no_responders(&e) => return WaitReadyResponse::Unsupported,
            Err(e) => {
                warn!(error = %e, lease_id = %req.lease_id, "wait_ready denied: worker dispatch failed");
                return WaitReadyResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };
        match serde_json::from_slice::<WaitReadyResponse>(&reply) {
            Ok(resp) => resp,
            Err(e) => WaitReadyResponse::Denied(LeaseError::Backend {
                message: format!("wait_ready reply decode failed: {e}"),
            }),
        }
    }

    /// Waits until the lease is `Running` and every requested port accepts a
    /// TCP connection inside the sandbox, bounded by the request's timeout.
    ///
    /// `Running` is only recorded after the worker's create reply, which
    /// follows workspace materialization, so it stands for "clone done". Ports
    /// are probed through [`Self::handle_run_command`] with a short
    /// synchronous script, so a provider that can exec needs no new verb and
    /// one that cannot reports [`WaitSandboxReadyResponse::Unsupported`].
    pub async fn handle_wait_sandbox_ready(&self, req: WaitSandboxReadyRequest) -> WaitSandboxReadyResponse {
        let deadline = tokio::time::Instant::now()
            + Duration::from_secs(u64::from(
                req.timeout_seconds.min(WaitSandboxReadyRequest::MAX_TIMEOUT_SECONDS),
            ));
        let mut pending = Vec::new();
        loop {
            let record = match self.active_leases.get(&req.lease_id).await {
                Ok(Some(r)) => r,
                Ok(None) => {
                    return WaitSandboxReadyResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
                }
                Err(e) => {
                    warn!(error = %e, lease_id = %req.lease_id, "wait_sandbox_ready denied: lease lookup failed");
                    return WaitSandboxReadyResponse::Denied(LeaseError::Backend { message: e.to_string() });
                }
            };
            match record.status {
                LeaseStatus::Pending => pending.clear(),
                LeaseStatus::Running => {
                    if req.ports.as_slice().is_empty() {
                        return WaitSandboxReadyResponse::Ready;
                    }
                    match self.probe_ports(&req.lease_id, req.ports.as_slice()).await {
                        PortProbe::Open => return WaitSandboxReadyResponse::Ready,
                        PortProbe::Closed(ports) => pending = ports,
                        PortProbe::Unsupported => return WaitSandboxReadyResponse::Unsupported,
                        PortProbe::Denied(e) => return WaitSandboxReadyResponse::Denied(e),
                        PortProbe::Retry => pending = req.ports.as_slice().to_vec(),
                    }
                }
                LeaseStatus::Paused
                | LeaseStatus::Suspending
                | LeaseStatus::Suspended
                | LeaseStatus::Waking
                | LeaseStatus::Cancelling
                | LeaseStatus::Released
                | LeaseStatus::Failed => {
                    return WaitSandboxReadyResponse::Denied(LeaseError::LeaseNotRunning { lease_id: req.lease_id });
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return WaitSandboxReadyResponse::NotReady { pending_ports: pending };
            }
            tokio::time::sleep(SANDBOX_READY_POLL_INTERVAL).await;
        }
    }

    async fn probe_ports(&self, lease_id: &LeaseId, ports: &[u16]) -> PortProbe {
        let mut command = vec![
            "sh".to_string(),
            "-c".to_string(),
            PORT_PROBE_SCRIPT.to_string(),
            "sh".to_string(),
        ];
        command.extend(ports.iter().map(u16::to_string));
        let response = self
            .handle_run_command(RunCommandRequest {
                require_launch_cancellation: LaunchCancellation::NotRequired,
                launch_key: None,
                lease_id: lease_id.clone(),
                command,
                cwd: None,
                env: Default::default(),
                timeout_seconds: PORT_PROBE_TIMEOUT_SECONDS,
                stream_output: StreamedOutput::Disabled,
                run_as_user: None,
                output_retention: Default::default(),
                max_output_bytes: Some(PORT_PROBE_MAX_OUTPUT_BYTES),
                redact_values: Default::default(),
                execution: CommandExecution::Attached,
            })
            .await;
        match response {
            RunCommandResponse::Completed(out) if out.exit_code == PORT_PROBE_NO_TOOL_EXIT => PortProbe::Unsupported,
            RunCommandResponse::Completed(out) if out.exit_code == 0 && out.timed_out.is_within_deadline() => {
                let closed: Vec<u16> = String::from_utf8_lossy(&out.stdout)
                    .split_whitespace()
                    .filter_map(|p| p.parse().ok())
                    .filter(|p| ports.contains(p))
                    .collect();
                if closed.is_empty() {
                    PortProbe::Open
                } else {
                    PortProbe::Closed(closed)
                }
            }
            RunCommandResponse::Denied(LeaseError::NoWorkerSatisfiesCapabilities { .. }) => PortProbe::Unsupported,
            RunCommandResponse::Denied(e @ (LeaseError::LeaseNotFound { .. } | LeaseError::LeaseNotRunning { .. })) => {
                PortProbe::Denied(e)
            }
            _ => PortProbe::Retry,
        }
    }
}
