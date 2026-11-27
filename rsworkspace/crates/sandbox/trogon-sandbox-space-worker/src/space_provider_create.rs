use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;
use trogon_sandbox_contract::{LeaseAttemptIdentity, LeaseId, LeaseRequest, ProviderLease};
use trogon_sandbox_protocol::subjects;

use crate::idempotency::{fresh, idempotency_key};
use crate::{
    Accepted, ApiError, LeaseRecord, Pending, SandboxInfo, SandboxState, SpaceId, SpaceProvider, SpaceProviderError,
};

impl SpaceProvider {
    pub(crate) fn receipt(id: &SpaceId, lease: &LeaseId) -> ProviderLease {
        ProviderLease {
            sandbox_id: id.sandbox_id().clone(),
            evidence_subject: subjects::evidence_subject(lease),
            worker_id: None,
            attempt_id: None,
            epoch: None,
        }
    }

    pub(crate) async fn lease_inner(
        &self,
        request: &LeaseRequest,
        lease: &LeaseId,
        identity: Option<&LeaseAttemptIdentity>,
    ) -> Result<ProviderLease, SpaceProviderError> {
        self.validate_lease(request)?;
        let pending = Arc::new(Pending::new());
        {
            let mut registry = self.registry.lock().await;
            let held = registry.active.len() + registry.pending.len();
            if registry.active.contains_key(lease.as_str())
                || registry.pending.contains_key(lease.as_str())
                || held >= self.config.max_leases.get() as usize
            {
                return Err(SpaceProviderError::Busy);
            }
            registry.pending.insert(lease.as_str().to_owned(), pending.clone());
        }
        let outcome = self.create_flow(request, lease, identity, &pending).await;
        let result = {
            let mut registry = self.registry.lock().await;
            registry.pending.remove(lease.as_str());
            outcome.map(|record| {
                let receipt = Self::receipt(&record.sandbox_id, lease);
                registry.active.insert(lease.as_str().to_owned(), record);
                receipt
            })
        };
        pending.finish();
        result
    }

    /// Creates the sandbox and waits until it is really usable. SPACE accepting the
    /// request is not readiness: the create operation must succeed, the sandbox must be
    /// running and the guest must answer a process query. A cancel at any point deletes
    /// what was made.
    async fn create_flow(
        &self,
        request: &LeaseRequest,
        lease: &LeaseId,
        identity: Option<&LeaseAttemptIdentity>,
        pending: &Pending,
    ) -> Result<LeaseRecord, SpaceProviderError> {
        let attempt = fresh();
        let nonce = identity.map_or_else(
            || attempt.clone(),
            |identity| identity.target.attempt_id.as_str().to_owned(),
        );
        let key = idempotency_key("create", &[lease.as_str(), &nonce]);
        let body = self.create_body(request, lease, &attempt, identity);
        let accepted = match self.api.create_sandbox(&body, &key).await {
            Ok(accepted) => accepted,
            Err(error) if error.definitely_not_applied() => return Err(error.into()),
            Err(error) => return Err(self.reconcile(&attempt, error).await),
        };
        let Some(id) = accepted.sandbox_id.clone() else {
            let error = ApiError::Protocol("create reply carried no sandbox ID".into());
            return Err(self.reconcile(&attempt, error).await);
        };
        pending.set_sandbox(id.clone());
        let ready = async {
            if let Some(identity) = identity {
                self.ledger
                    .record_backend_receipt(identity, Self::receipt(&id, lease))
                    .await?;
            }
            pending.ensure_live()?;
            self.wait_operation(&accepted, self.config.boot_timeout, Some(pending))
                .await?;
            self.wait_state(&id, &SandboxState::Running, self.config.boot_timeout, Some(pending))
                .await?;
            pending.ensure_live()?;
            self.api.probe_processes(&id, self.config.boot_timeout).await?;
            pending.ensure_live()
        }
        .await;
        if let Err(error) = ready {
            return Err(match self.destroy(&id).await {
                Ok(()) => error,
                Err(cleanup) => SpaceProviderError::CleanupFailed {
                    error: Box::new(error),
                    cleanup: Box::new(cleanup),
                },
            });
        }
        Ok(LeaseRecord {
            sandbox_id: id,
            cwd: request.spec.working_dir().map(str::to_owned),
            env: request.spec.env().clone(),
        })
    }

    /// A create of unknown outcome may have landed. Reclaim only this attempt's sandbox.
    async fn reconcile(&self, attempt: &str, error: ApiError) -> SpaceProviderError {
        match self.reclaim_attempt(attempt).await {
            Ok(()) => error.into(),
            Err(failure) => SpaceProviderError::Reconcile(Box::new(failure)),
        }
    }

    pub(crate) async fn wait_operation(
        &self,
        accepted: &Accepted,
        limit: Duration,
        pending: Option<&Pending>,
    ) -> Result<(), SpaceProviderError> {
        let deadline = Instant::now() + limit;
        let mut known = accepted.operation.clone().filter(crate::Operation::is_terminal);
        loop {
            let operation = match known.take() {
                Some(operation) => operation,
                None => {
                    if let Some(pending) = pending {
                        pending.ensure_live()?;
                    }
                    self.api.operation(&accepted.operation_id).await?
                }
            };
            if operation.is_terminal() {
                return if operation.succeeded() {
                    Ok(())
                } else {
                    Err(SpaceProviderError::OperationFailed(operation.failure()))
                };
            }
            if Instant::now() >= deadline {
                return Err(SpaceProviderError::OperationTimeout {
                    id: accepted.operation_id.clone(),
                    state: operation.state().clone(),
                });
            }
            tokio::time::sleep(self.config.poll_interval).await;
        }
    }

    pub(crate) async fn wait_state(
        &self,
        id: &SpaceId,
        target: &SandboxState,
        limit: Duration,
        pending: Option<&Pending>,
    ) -> Result<(), SpaceProviderError> {
        let deadline = Instant::now() + limit;
        loop {
            if let Some(pending) = pending {
                pending.ensure_live()?;
            }
            let info = self.api.sandbox(id).await?.ok_or(SpaceProviderError::Disappeared)?;
            if info.state == *target {
                return Ok(());
            }
            if !info.state.is_transitional() {
                return Err(SpaceProviderError::UnexpectedState {
                    failure: info.failure_code.unwrap_or_else(|| "none".to_owned()),
                    state: info.state,
                    target: target.as_str().to_owned(),
                });
            }
            if Instant::now() >= deadline {
                return Err(SpaceProviderError::StateTimeout {
                    target: target.as_str().to_owned(),
                    state: info.state,
                });
            }
            tokio::time::sleep(self.config.poll_interval).await;
        }
    }

    async fn wait_gone(&self, id: &SpaceId, deadline: Instant) -> Result<(), SpaceProviderError> {
        loop {
            match self.api.sandbox(id).await? {
                None => return Ok(()),
                Some(info) if info.state == SandboxState::Deleted => return Ok(()),
                Some(info) => {
                    if Instant::now() >= deadline {
                        return Err(SpaceProviderError::DeleteTimeout { state: info.state });
                    }
                    tokio::time::sleep(self.config.poll_interval).await;
                }
            }
        }
    }

    /// Deletes a sandbox and waits until it is gone. Delete is accepted in any state, so
    /// it also ends a create that is still in flight; a refusal because the sandbox is
    /// mid-transition is retried once the state settles.
    pub async fn destroy(&self, id: &SpaceId) -> Result<(), SpaceProviderError> {
        let deadline = Instant::now() + self.config.operation_timeout;
        loop {
            let key = idempotency_key("delete", &[id.as_str(), &fresh()]);
            match self.api.delete_sandbox(id, &key).await {
                Ok(None) => return Ok(()),
                Ok(Some(accepted)) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    self.wait_operation(&accepted, left, None).await?;
                    return self.wait_gone(id, deadline).await;
                }
                Err(error)
                    if matches!(error.code(), Some("conflict" | "failed_precondition"))
                        && Instant::now() < deadline =>
                {
                    match self.api.sandbox(id).await? {
                        None => return Ok(()),
                        Some(info) if info.state == SandboxState::Deleted => return Ok(()),
                        Some(info) if info.state == SandboxState::Deleting => {
                            return self.wait_gone(id, deadline).await;
                        }
                        Some(_) => tokio::time::sleep(self.config.poll_interval).await,
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    pub(crate) async fn owned_sandboxes(&self) -> Result<Vec<SandboxInfo>, SpaceProviderError> {
        let mut found = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let (page, next) = self.api.list_sandboxes(cursor.as_deref()).await?;
            found.extend(page.into_iter().filter(|sandbox| {
                sandbox.metadata_text("sandbox_owner") == Some(self.owner.as_str())
                    && sandbox.state != SandboxState::Deleted
            }));
            match next {
                Some(next) => cursor = Some(next),
                None => return Ok(found),
            }
        }
    }

    async fn reclaim_attempt(&self, attempt: &str) -> Result<(), SpaceProviderError> {
        let owned = self.owned_sandboxes().await?;
        for sandbox in owned
            .iter()
            .filter(|sandbox| sandbox.metadata_text("sandbox_attempt") == Some(attempt))
        {
            self.destroy(&sandbox.id).await?;
        }
        Ok(())
    }
}
