use super::*;
use crate::profile_satisfaction::ProfileSatisfies;
use trogon_sandbox_contract::{CapabilityNeed, FileOp};
use trogon_sandbox_protocol::{file_op_token, unsupported_file_op_response};

impl BrokerCore {
    /// Writes one or more files into an already-running lease's sandbox via
    /// the owning worker. Routed like [`Self::handle_run_command`].
    pub async fn handle_write_files(&self, req: WriteFilesRequest) -> WriteFilesResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "write_files denied: lease not found");
                return WriteFilesResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "write_files denied: active lease lookup failed");
                return WriteFilesResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id, status = ?record.status, "write_files denied: lease is not running");
            return WriteFilesResponse::Denied(self.not_running_error(&record).await);
        }

        let lease_id = req.lease_id.clone();
        let sent_bytes: u64 = req.files.iter().map(|f| f.content.len() as u64).sum();
        let body = match encode_write_files_within(&req, trogon_sandbox_protocol::nats_transfer::MAX_TRANSFER_BYTES) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "write_files denied before dispatch");
                return WriteFilesResponse::Denied(e);
            }
        };

        self.touch_activity(&record, false).await;
        let subject = trogon_sandbox_protocol::subjects::provider_write_files(record.provider, &lease_id);
        let reply = match self.bus.request(&subject, body).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "write_files denied: worker dispatch failed");
                return WriteFilesResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match serde_json::from_slice::<WriteFilesResponse>(&reply) {
            Ok(resp) => {
                if matches!(resp, WriteFilesResponse::Written) {
                    self.count_usage(
                        &record,
                        LeaseUsageCounters {
                            file_bytes_in: sent_bytes,
                            ..Default::default()
                        },
                    )
                    .await;
                }
                resp
            }
            Err(e) => WriteFilesResponse::Denied(LeaseError::Backend {
                message: format!("write_files reply decode failed: {e}"),
            }),
        }
    }

    /// Reads a single file out of an already-running lease's sandbox via the
    /// owning worker. Routed like [`Self::handle_run_command`].
    pub async fn handle_read_file(&self, req: ReadFileRequest) -> ReadFileResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "read_file denied: lease not found");
                return ReadFileResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "read_file denied: active lease lookup failed");
                return ReadFileResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id, status = ?record.status, "read_file denied: lease is not running");
            return ReadFileResponse::Denied(self.not_running_error(&record).await);
        }

        let lease_id = req.lease_id.clone();
        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return ReadFileResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        self.touch_activity(&record, false).await;
        let subject = trogon_sandbox_protocol::subjects::provider_read_file(record.provider, &lease_id);
        let reply = match self.bus.request(&subject, body).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, "read_file denied: worker dispatch failed");
                return ReadFileResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match serde_json::from_slice::<ReadFileResponse>(&reply) {
            Ok(resp) => {
                if let ReadFileResponse::Content { data } = &resp {
                    self.count_usage(
                        &record,
                        LeaseUsageCounters {
                            file_bytes_out: data.len() as u64,
                            ..Default::default()
                        },
                    )
                    .await;
                }
                resp
            }
            Err(e) => ReadFileResponse::Denied(LeaseError::Backend {
                message: format!("read_file reply decode failed: {e}"),
            }),
        }
    }

    /// Runs one structured file operation (list, stat, ranged read, edit, search, remove,
    /// mkdir, rename, archive) inside an already-running lease's sandbox via the owning
    /// worker. Routed like [`Self::handle_read_file`]; a worker that does not serve the verb
    /// answers a typed `Unsupported` reply.
    pub async fn handle_file_op(&self, req: FileOpRequest) -> FileOpResponse {
        let record = match self.active_leases.get(&req.lease_id).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                warn!(lease_id = %req.lease_id, "file_op denied: lease not found");
                return FileOpResponse::Denied(LeaseError::LeaseNotFound { lease_id: req.lease_id });
            }
            Err(e) => {
                warn!(error = %e, "file_op denied: active lease lookup failed");
                return FileOpResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        if record.status != LeaseStatus::Running {
            warn!(lease_id = %req.lease_id, status = ?record.status, "file_op denied: lease is not running");
            return FileOpResponse::Denied(self.not_running_error(&record).await);
        }

        // Tracking, watch and checksum verbs are optional on top of the plain file verbs, so a
        // provider that does not advertise them is refused here instead of mis-serving them.
        let tracking = is_change_tracking(&req.op);
        let watch = is_file_watch(&req.op);
        let transfer = is_dir_transfer(&req.op);
        if tracking || watch || transfer {
            let requirements = CapabilityRequirements {
                needs_change_tracking: CapabilityNeed::from(tracking),
                needs_file_watch: CapabilityNeed::from(watch),
                needs_dir_transfer: CapabilityNeed::from(transfer),
                ..Default::default()
            };
            if let Some(profile) = self.worker_registry.profile_for_provider(record.provider).await
                && !profile.satisfies(&requirements)
            {
                return unsupported_file_op_response(&format!("{:?}", record.provider), &req.op);
            }
        }

        let lease_id = req.lease_id.clone();
        let body = match serde_json::to_vec(&req) {
            Ok(b) => Bytes::from(b),
            Err(e) => {
                return FileOpResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        let subject = trogon_sandbox_protocol::subjects::provider_file_op(record.provider, &lease_id);
        let reply = match self.bus.request(&subject, body).await {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, lease_id = %lease_id, op = file_op_token(&req.op), "file_op denied: worker dispatch failed");
                return FileOpResponse::Denied(LeaseError::Backend { message: e.to_string() });
            }
        };

        match serde_json::from_slice::<FileOpResponse>(&reply) {
            Ok(resp) => {
                let delta = match &resp {
                    FileOpResponse::Content { data, .. } => Some(LeaseUsageCounters {
                        file_bytes_out: data.as_str().len() as u64,
                        ..Default::default()
                    }),
                    FileOpResponse::Written { bytes, .. } => Some(LeaseUsageCounters {
                        file_bytes_in: *bytes,
                        ..Default::default()
                    }),
                    _ => None,
                };
                if let Some(delta) = delta {
                    self.count_usage(&record, delta).await;
                }
                resp
            }
            Err(e) => FileOpResponse::Denied(LeaseError::Backend {
                message: format!("file_op reply decode failed: {e}"),
            }),
        }
    }
}

/// Encodes a write request, refusing a body the bus cannot carry.
fn encode_write_files_within(req: &WriteFilesRequest, max_payload: usize) -> Result<Vec<u8>, LeaseError> {
    let body = serde_json::to_vec(req).map_err(|e| LeaseError::Backend { message: e.to_string() })?;
    if body.len() > max_payload {
        return Err(LeaseError::FilesTooLargeForTransport {
            content_bytes: req.files.iter().map(|f| f.content.len() as u64).sum(),
            max_payload_bytes: max_payload as u64,
        });
    }
    Ok(body)
}

/// True for the integrity verb a directory transfer relies on.
fn is_dir_transfer(op: &FileOp) -> bool {
    matches!(op, FileOp::Checksum { .. })
}

/// True for the change-tracking verbs, which a profile advertises separately from the plain file verbs.
fn is_change_tracking(op: &FileOp) -> bool {
    matches!(op, FileOp::Checkpoint { .. } | FileOp::Changes { .. })
}

/// True for the watch verbs, advertised separately like change tracking.
fn is_file_watch(op: &FileOp) -> bool {
    matches!(
        op,
        FileOp::WatchStart { .. } | FileOp::WatchPoll { .. } | FileOp::WatchStop { .. }
    )
}
