use serde::Deserialize;
use serde_json::Value;
use trogon_sandbox_contract::{
    FileOpRequest, GetSandboxDomainResponse, LeaseError, LeaseId, ReadFileRequest, RunCommandRequest,
    SandboxSuspendRequest, SandboxWakeRequest, SnapshotExportError, SnapshotExportOutcome, SuspendLevel,
    WriteFilesRequest, WriteFilesResponse,
};
use trogon_sandbox_protocol::{read_file_response, unsupported_file_op_response};

use crate::freeze_replies::{suspend_reply, wake_reply};
use crate::{LifecycleAction, OperationDenied, OperationError, OperationSubject, OperationVerb, SpaceProvider};

#[derive(Deserialize)]
struct Envelope {
    lease_id: LeaseId,
}

fn reply<T: serde::Serialize>(value: &T) -> Result<Value, OperationError> {
    Ok(serde_json::to_value(value)?)
}

impl SpaceProvider {
    /// Whether an operation message is this worker's to answer: the lease is held and the
    /// verb is a request rather than a lease announcement.
    pub async fn serves_operation(&self, subject: &OperationSubject) -> bool {
        !matches!(subject.verb(), OperationVerb::Cancel | OperationVerb::Purge)
            && self.holds_lease(subject.lease()).await
    }

    /// Answers one operation request for a held lease.
    pub async fn answer_operation(&self, subject: &OperationSubject, payload: &[u8]) -> Result<Value, OperationError> {
        let envelope: Envelope = serde_json::from_slice(payload)?;
        if envelope.lease_id != *subject.lease() {
            return Err(OperationError::LeaseMismatch);
        }
        let lease = subject.lease();
        match subject.verb() {
            OperationVerb::CheckpointRunning | OperationVerb::Snapshot => {
                reply(&SnapshotExportOutcome::Err(SnapshotExportError::Unsupported {
                    message: "this worker does not use SPACE snapshots; suspend keeps a sandbox durable".into(),
                }))
            }
            OperationVerb::GetDomain => reply(&GetSandboxDomainResponse::PortNotExposed),
            OperationVerb::Pause => reply(&self.set_dormancy(lease, LifecycleAction::Pause).await),
            OperationVerb::Resume => reply(&self.set_dormancy(lease, LifecycleAction::Wake).await),
            OperationVerb::Suspend => {
                let request: SandboxSuspendRequest = serde_json::from_slice(payload)?;
                reply(&if request.level == SuspendLevel::Memory {
                    suspend_reply(self.set_dormancy(lease, LifecycleAction::Suspend).await)
                } else {
                    trogon_sandbox_contract::SandboxSuspendResponse::Unsupported
                })
            }
            OperationVerb::Wake => {
                let request: SandboxWakeRequest = serde_json::from_slice(payload)?;
                reply(&if request.level == SuspendLevel::Memory {
                    wake_reply(self.set_dormancy(lease, LifecycleAction::Wake).await)
                } else {
                    trogon_sandbox_contract::SandboxWakeResponse::Unsupported
                })
            }
            OperationVerb::RunCommand => {
                let request: RunCommandRequest = serde_json::from_slice(payload)?;
                reply(&self.run_command(&request).await)
            }
            OperationVerb::WriteFiles => {
                let request: WriteFilesRequest = serde_json::from_slice(payload)?;
                reply(&match self.write_files(&request).await {
                    Ok(()) => WriteFilesResponse::Written,
                    Err(error) => WriteFilesResponse::Denied(LeaseError::Backend {
                        message: error.to_string(),
                    }),
                })
            }
            OperationVerb::ReadFile => {
                let request: ReadFileRequest = serde_json::from_slice(payload)?;
                reply(&read_file_response(self.read_file(&request).await, |error| {
                    LeaseError::Backend {
                        message: error.to_string(),
                    }
                }))
            }
            OperationVerb::FileOp => {
                let request: FileOpRequest = serde_json::from_slice(payload)?;
                reply(&unsupported_file_op_response("space", &request.op))
            }
            verb @ (OperationVerb::Cancel | OperationVerb::Purge | OperationVerb::Other(_)) => {
                Ok(serde_json::to_value(OperationDenied {
                    outcome: "denied",
                    kind: "invalid_spec",
                    reason: format!("{} is unavailable on the SPACE worker", verb.as_str()),
                })?)
            }
        }
    }
}
