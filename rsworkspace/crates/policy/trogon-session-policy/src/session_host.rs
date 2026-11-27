use std::convert::Infallible;

use crate::{
    AuditSink, Denial, DenialReason, ModelExecutor, ModelPurpose, Operation, OperationError, PolicyFingerprint,
    ReviewExecutor, ReviewPolicy, ReviewVeto, SessionId, SessionPolicy, ShellError, ShellExecutor, ToolId,
    WasmExecutor,
};

/// The trusted owner of one policy and its effect adapters.
///
/// Keep this value outside the agent. Bind it to authenticated session transport.
/// Neither policy replacement nor unmediated adapter access is exposed.
pub struct SessionHost<M, W, S, R, A> {
    session: SessionId,
    policy: SessionPolicy,
    models: M,
    wasm: W,
    shell: S,
    reviewer: R,
    audit: A,
    closed: bool,
    model_calls: u32,
    tool_calls: u32,
}

impl<M, W, S, R, A: AuditSink> SessionHost<M, W, S, R, A> {
    pub fn new(session: SessionId, policy: SessionPolicy, models: M, wasm: W, shell: S, reviewer: R, audit: A) -> Self {
        Self {
            session,
            policy,
            models,
            wasm,
            shell,
            reviewer,
            audit,
            closed: false,
            model_calls: 0,
            tool_calls: 0,
        }
    }

    pub fn fingerprint(&self) -> PolicyFingerprint {
        self.policy.fingerprint()
    }

    /// Revocation is terminal for this host, including requests already queued by a caller.
    pub fn close(&mut self) {
        self.closed = true;
    }

    /// Management cannot replace the snapshot in a live session.
    pub async fn require_same_policy(
        &mut self,
        fingerprint: PolicyFingerprint,
    ) -> Result<(), OperationError<Infallible, A::Error>> {
        let reason = if self.closed {
            Some(DenialReason::SessionClosed)
        } else if fingerprint != self.policy.fingerprint() {
            Some(DenialReason::NewSessionRequired)
        } else {
            None
        };
        if let Some(reason) = reason {
            return Err(self.refuse(Operation::PolicyChange, reason).await);
        }
        Ok(())
    }

    fn denial(&self, operation: Operation, reason: DenialReason) -> Denial {
        Denial {
            session: self.session.clone(),
            policy: self.policy.fingerprint(),
            operation,
            reason,
        }
    }

    async fn refuse<E: std::error::Error>(
        &mut self,
        operation: Operation,
        reason: DenialReason,
    ) -> OperationError<E, A::Error> {
        let denial = self.denial(operation, reason);
        match self.audit.record_denial(&denial).await {
            Ok(()) => OperationError::Refused(denial),
            Err(error) => OperationError::Audit(error),
        }
    }

    fn basic_denial(&self, content: &str) -> Option<DenialReason> {
        if self.closed {
            Some(DenialReason::SessionClosed)
        } else if content.len() > self.policy.limits().payload_bytes() as usize {
            Some(DenialReason::PayloadLimit)
        } else {
            None
        }
    }

    pub async fn model(
        &mut self,
        purpose: ModelPurpose,
        content: &str,
    ) -> Result<M::Output, OperationError<M::Error, A::Error>>
    where
        M: ModelExecutor,
    {
        let reason = self
            .basic_denial(content)
            .or_else(|| (self.policy.route(purpose).is_none()).then_some(DenialReason::RouteUndeclared))
            .or_else(|| {
                (self.model_calls >= self.policy.limits().model_calls()).then_some(DenialReason::BudgetExhausted)
            });
        if let Some(reason) = reason {
            return Err(self.refuse(Operation::Model(purpose), reason).await);
        }
        let Some(route) = self.policy.route(purpose) else {
            return Err(self
                .refuse(Operation::Model(purpose), DenialReason::RouteUndeclared)
                .await);
        };
        self.model_calls += 1;
        self.models
            .model(purpose, route, content)
            .await
            .map_err(OperationError::Effect)
    }

    pub async fn wasm(
        &mut self,
        tool: &ToolId,
        content: &str,
    ) -> Result<W::Output, ShellError<W::Error, R::Error, A::Error>>
    where
        W: WasmExecutor,
        R: ReviewExecutor,
    {
        let reason = self
            .basic_denial(content)
            .or_else(|| (self.policy.tool(tool).is_none()).then_some(DenialReason::ToolUndeclared))
            .or_else(|| {
                (self.tool_calls >= self.policy.limits().tool_calls()).then_some(DenialReason::BudgetExhausted)
            });
        if let Some(reason) = reason {
            return Err(self.tool_refusal(Operation::WasmTool, reason).await);
        }
        let Some(tool) = self.policy.tool(tool).cloned() else {
            return Err(self
                .tool_refusal(Operation::WasmTool, DenialReason::ToolUndeclared)
                .await);
        };
        self.tool_calls += 1;
        self.review(Operation::WasmTool, tool.review, content).await?;
        self.wasm.wasm(&tool, content).await.map_err(ShellError::Effect)
    }

    pub async fn shell(&mut self, command: &str) -> Result<S::Output, ShellError<S::Error, R::Error, A::Error>>
    where
        S: ShellExecutor,
        R: ReviewExecutor,
    {
        let reason = self
            .basic_denial(command)
            .or_else(|| {
                (command.is_empty() || command.len() > 8192 || command.contains('\0'))
                    .then_some(DenialReason::PayloadLimit)
            })
            .or_else(|| (self.policy.shell().is_none()).then_some(DenialReason::ShellDisabled))
            .or_else(|| {
                (self.tool_calls >= self.policy.limits().tool_calls()).then_some(DenialReason::BudgetExhausted)
            });
        if let Some(reason) = reason {
            return Err(self.tool_refusal(Operation::Shell, reason).await);
        }
        let Some(grant) = self.policy.shell().cloned() else {
            return Err(self.tool_refusal(Operation::Shell, DenialReason::ShellDisabled).await);
        };
        self.tool_calls += 1;
        self.review(Operation::Shell, grant.review, command).await?;
        self.shell.shell(&grant, command).await.map_err(ShellError::Effect)
    }

    async fn tool_refusal<E: std::error::Error, V: std::error::Error>(
        &mut self,
        operation: Operation,
        reason: DenialReason,
    ) -> ShellError<E, V, A::Error> {
        let denial = self.denial(operation, reason);
        match self.audit.record_denial(&denial).await {
            Ok(()) => ShellError::Refused(denial),
            Err(error) => ShellError::Audit(error),
        }
    }

    async fn review<E: std::error::Error>(
        &mut self,
        operation: Operation,
        policy: ReviewPolicy,
        content: &str,
    ) -> Result<(), ShellError<E, R::Error, A::Error>>
    where
        R: ReviewExecutor,
    {
        if policy == ReviewPolicy::DeterministicOnly {
            return Ok(());
        }
        if self.model_calls >= self.policy.limits().model_calls() {
            return Err(self.tool_refusal(operation, DenialReason::BudgetExhausted).await);
        }
        let Some(route) = self.policy.route(ModelPurpose::Reviewer) else {
            return Err(self.tool_refusal(operation, DenialReason::RouteUndeclared).await);
        };
        self.model_calls += 1;
        match self.reviewer.review(route, operation, content).await {
            Ok(ReviewVeto::NoVeto) => Ok(()),
            Ok(ReviewVeto::Veto) => Err(self.tool_refusal(operation, DenialReason::ReviewVeto).await),
            Err(error) => {
                let denial = self.denial(operation, DenialReason::ReviewUnavailable);
                self.audit.record_denial(&denial).await.map_err(ShellError::Audit)?;
                Err(ShellError::Review(error))
            }
        }
    }
}
