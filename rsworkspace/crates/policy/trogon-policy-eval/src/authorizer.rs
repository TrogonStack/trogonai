use std::sync::Arc;

use trogon_decider_runtime::{AuthorizationDeniedError, CommandAuthorizer, CommandPrincipal};
use trogon_std::time::{EpochClock, SystemClock};

use crate::action_mapper::ActionMapper;
use crate::decision_recorder::DecisionRecorder;
use crate::effective::Effective;
use crate::evaluator::Evaluator;
use crate::view::PolicyView;

/// A decider `CommandAuthorizer` backed by the policy evaluator.
///
/// It stays inside the seam of ADR#0026: synchronous, no policy language, and a
/// missing principal is rejected by the trait's own fail-closed default before
/// anything here runs. It maps a principal and a command to a request through an
/// [`ActionMapper`], evaluates it against the view, and turns a refusal into a
/// denial whose reason names the one binding that caused it.
///
/// Building the principal from verified credentials stays the host's job
/// (ADR#0026 Decision 4).
pub struct PolicyAuthorizer<M, V, K = SystemClock> {
    mapper: M,
    view: V,
    evaluator: Evaluator,
    clock: K,
    recorder: Option<Arc<dyn DecisionRecorder>>,
}

impl<M, V> PolicyAuthorizer<M, V, SystemClock> {
    /// Creates an authorizer that reads the system clock.
    pub fn new(mapper: M, view: V, evaluator: Evaluator) -> Self {
        Self {
            mapper,
            view,
            evaluator,
            clock: SystemClock,
            recorder: None,
        }
    }
}

impl<M, V, K> PolicyAuthorizer<M, V, K> {
    /// Replaces the clock that supplies `now` to each evaluation.
    pub fn with_clock<K2>(self, clock: K2) -> PolicyAuthorizer<M, V, K2> {
        PolicyAuthorizer {
            mapper: self.mapper,
            view: self.view,
            evaluator: self.evaluator,
            clock,
            recorder: self.recorder,
        }
    }

    /// Sends every decision to a recorder, including the refusals a `DryRun`
    /// evaluator admits.
    pub fn with_recorder(mut self, recorder: Arc<dyn DecisionRecorder>) -> Self {
        self.recorder = Some(recorder);
        self
    }
}

impl<C, M, V, K> CommandAuthorizer<C> for PolicyAuthorizer<M, V, K>
where
    C: ?Sized,
    M: ActionMapper<C>,
    V: PolicyView,
    K: EpochClock,
{
    fn authorize(&self, principal: &CommandPrincipal, command: &C) -> Result<(), AuthorizationDeniedError> {
        let request = self.mapper.request(principal, command).map_err(|error| {
            AuthorizationDeniedError::new(format!("command cannot be mapped to a policy action: {error}"))
        })?;
        let decision = self
            .evaluator
            .evaluate(&self.view, &request, &[], self.clock.system_time());
        if let Some(recorder) = &self.recorder {
            recorder.record(principal, &request, &decision);
        }
        match decision.effective() {
            Effective::Admit => Ok(()),
            Effective::Refuse(refusal) => Err(AuthorizationDeniedError::new(refusal.to_string())),
        }
    }
}
