use std::sync::Arc;

use async_trait::async_trait;

use crate::approvals::coordinator::ApprovalCoordinator;
use crate::approvals::errors::ApprovalError;
use crate::approvals::types::{ApprovalDecision, ApprovalRequest};

#[async_trait]
pub trait ApprovalGate: Send + Sync {
    async fn request_approval(
        &self,
        request_ctx: &ApprovalRequest,
    ) -> Result<ApprovalDecision, ApprovalError>;
}

pub struct CoordinatorApprovalGate {
    coordinator: Arc<ApprovalCoordinator>,
}

impl CoordinatorApprovalGate {
    #[must_use]
    pub fn new(coordinator: Arc<ApprovalCoordinator>) -> Self {
        Self { coordinator }
    }
}

#[async_trait]
impl ApprovalGate for CoordinatorApprovalGate {
    async fn request_approval(
        &self,
        request_ctx: &ApprovalRequest,
    ) -> Result<ApprovalDecision, ApprovalError> {
        self.coordinator
            .wait_for_decision(&request_ctx.request_id, request_ctx.ttl)
            .await
    }
}
