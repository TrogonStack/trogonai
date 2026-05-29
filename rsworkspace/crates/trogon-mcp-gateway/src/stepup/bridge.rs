//! Service-workload escalation into the approvals flow (wired by a future PR).

use async_trait::async_trait;

use super::errors::StepUpError;
use super::policy::StepUpRequestCtx;

#[async_trait]
pub trait ApprovalBridge: Send + Sync {
    async fn escalate(&self, request_ctx: &StepUpRequestCtx) -> Result<(), StepUpError>;
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NoopApprovalBridge;

#[async_trait]
impl ApprovalBridge for NoopApprovalBridge {
    async fn escalate(&self, _request_ctx: &StepUpRequestCtx) -> Result<(), StepUpError> {
        Ok(())
    }
}
