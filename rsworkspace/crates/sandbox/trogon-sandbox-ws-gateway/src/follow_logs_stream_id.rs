use trogon_sandbox_contract::{ExecutionId, LeaseId};

/// The stream id log events of one execution travel under.
pub fn follow_logs_stream_id(lease_id: &LeaseId, execution_id: &ExecutionId) -> String {
    format!("logs:{}:{}", lease_id.as_str(), execution_id.as_str())
}
