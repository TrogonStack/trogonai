use trogon_sandbox_contract::{OperationKind, OperationRequest, SandboxImage};

/// What an operation request does.
///
/// A create whose image is a snapshot is a restore.
pub fn operation_request_kind(request: &OperationRequest) -> OperationKind {
    match request {
        OperationRequest::Create { request, .. } => {
            if matches!(request.spec.image(), SandboxImage::Snapshot { .. }) {
                OperationKind::Restore
            } else {
                OperationKind::Create
            }
        }
        OperationRequest::Pause { .. } => OperationKind::Pause,
        OperationRequest::Resume => OperationKind::Resume,
        OperationRequest::Suspend { .. } => OperationKind::Suspend,
        OperationRequest::Wake => OperationKind::Wake,
        OperationRequest::Delete => OperationKind::Delete,
        OperationRequest::Rollback { .. } => OperationKind::Rollback,
    }
}
