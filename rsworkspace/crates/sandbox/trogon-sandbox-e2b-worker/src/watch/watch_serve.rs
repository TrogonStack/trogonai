use trogon_sandbox_contract::{FileOp, FileOpResponse, LeaseId};
use trogon_sandbox_protocol::unsupported_file_op_response;

use super::constants::DEFAULT_USER;
use super::watches::Watches;
use crate::envd::EnvdTarget;

impl Watches {
    /// Serves a watch verb. Any other verb is answered `Unsupported`.
    pub async fn serve(
        &self,
        lease: &LeaseId,
        target: &EnvdTarget,
        as_user: Option<&str>,
        op: &FileOp,
    ) -> FileOpResponse {
        let lease_key = lease.as_str();
        self.expire_idle(lease_key, target).await;
        let user = as_user.unwrap_or(DEFAULT_USER);
        let outcome = match op {
            FileOp::WatchStart { path, recursive } => self.start(lease_key, target, user, path, *recursive).await,
            FileOp::WatchPoll {
                watch_id,
                after_seq,
                wait_ms,
            } => {
                let response = self.poll(lease_key, target, watch_id, *after_seq, *wait_ms).await;
                self.publish_overflow(lease, watch_id).await;
                response
            }
            FileOp::WatchStop { watch_id } => Ok(self.stop(lease_key, target, watch_id).await),
            other => Ok(unsupported_file_op_response("e2b", other)),
        };
        outcome.unwrap_or_else(FileOpResponse::from)
    }
}
