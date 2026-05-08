use bytes::Bytes;
use futures_util::future::join_all;
use trogon_registry::{AgentCapability, Registry, RegistryStore};

use crate::{
    caller::AgentCaller,
    provider::OrchestratorProvider,
    types::{OrchestrationResult, OrchestratorError, SubTask, SubTaskResult, TaskPlan},
};

/// Orchestrates a top-level task by:
/// 1. Listing available agents from the registry.
/// 2. Calling the LLM to decompose the task into parallel sub-tasks.
/// 3. Dispatching all sub-tasks concurrently via `AgentCaller`.
/// 4. Calling the LLM again to synthesize all results.
pub struct OrchestratorEngine<P: OrchestratorProvider, C: AgentCaller, S: RegistryStore> {
    provider: P,
    caller: C,
    registry: Registry<S>,
}

impl<P: OrchestratorProvider, C: AgentCaller, S: RegistryStore> OrchestratorEngine<P, C, S> {
    pub fn new(provider: P, caller: C, registry: Registry<S>) -> Self {
        Self { provider, caller, registry }
    }

    /// Run a full orchestration cycle for `task`.
    pub async fn orchestrate(&self, task: &str) -> Result<OrchestrationResult, OrchestratorError> {
        let capabilities = self
            .registry
            .list_all()
            .await
            .map_err(|e| OrchestratorError::Planning(e.to_string()))?;

        let plan = self.provider.plan_task(task, &capabilities).await?;

        let sub_results = self.execute_plan(&plan, &capabilities).await;

        let synthesis =
            self.provider.synthesize_results(task, &plan, &sub_results).await?;

        Ok(OrchestrationResult {
            task: task.to_string(),
            plan,
            sub_results,
            synthesis,
        })
    }

    async fn execute_plan(
        &self,
        plan: &TaskPlan,
        capabilities: &[AgentCapability],
    ) -> Vec<SubTaskResult> {
        let futures: Vec<_> = plan
            .subtasks
            .iter()
            .map(|subtask| self.execute_subtask(subtask, capabilities))
            .collect();

        join_all(futures).await
    }

    async fn execute_subtask(
        &self,
        subtask: &SubTask,
        capabilities: &[AgentCapability],
    ) -> SubTaskResult {
        let cap = capabilities
            .iter()
            .find(|c| c.has_capability(&subtask.capability));

        let Some(cap) = cap else {
            return SubTaskResult::err(
                &subtask.id,
                &subtask.capability,
                format!("no agent registered for capability '{}'", subtask.capability),
            );
        };

        match self.caller.call(cap, Bytes::from(subtask.payload.clone())).await {
            Ok(output) => SubTaskResult::ok(&subtask.id, &subtask.capability, output.to_vec()),
            Err(e) => SubTaskResult::err(&subtask.id, &subtask.capability, e.to_string()),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        caller::mock::MockAgentCaller,
        types::{OrchestratorError, SubTask, TaskPlan},
    };
    use std::future::Future;
    use trogon_registry::AgentCapability;

    // ── Mock provider ─────────────────────────────────────────────────────────

    #[derive(Clone)]
    struct MockOrchestratorProvider {
        plan: Option<TaskPlan>,
        synthesis: Option<String>,
    }

    impl MockOrchestratorProvider {
        fn returning(plan: TaskPlan, synthesis: impl Into<String>) -> Self {
            Self { plan: Some(plan), synthesis: Some(synthesis.into()) }
        }
    }

    impl OrchestratorProvider for MockOrchestratorProvider {
        fn plan_task<'a>(
            &'a self,
            _task: &'a str,
            _capabilities: &'a [AgentCapability],
        ) -> impl Future<Output = Result<TaskPlan, OrchestratorError>> + Send + 'a {
            let plan = self.plan.clone();
            async move { plan.ok_or_else(|| OrchestratorError::Planning("no plan".into())) }
        }

        fn synthesize_results<'a>(
            &'a self,
            _task: &'a str,
            _plan: &'a TaskPlan,
            _results: &'a [SubTaskResult],
        ) -> impl Future<Output = Result<String, OrchestratorError>> + Send + 'a {
            let synthesis = self.synthesis.clone();
            async move {
                synthesis.ok_or_else(|| OrchestratorError::Synthesis("no synthesis".into()))
            }
        }
    }

    // ── Mock registry store ────────────────────────────────────────────────────

    use trogon_registry::MockRegistryStore;

    async fn engine_with(
        caps: Vec<AgentCapability>,
        plan: TaskPlan,
        synthesis: &str,
        caller: MockAgentCaller,
    ) -> OrchestratorEngine<MockOrchestratorProvider, MockAgentCaller, MockRegistryStore> {
        let store = MockRegistryStore::new();
        let registry = Registry::new(store.clone());
        for cap in caps {
            registry.register(&cap).await.unwrap();
        }
        let provider = MockOrchestratorProvider::returning(plan, synthesis);
        OrchestratorEngine::new(provider, caller, registry)
    }

    fn plan_with(subtasks: Vec<SubTask>) -> TaskPlan {
        TaskPlan { subtasks, reasoning: "test reasoning".into() }
    }

    fn subtask(id: &str, capability: &str) -> SubTask {
        SubTask {
            id: id.into(),
            capability: capability.into(),
            description: "do something".into(),
            payload: b"{}".to_vec(),
        }
    }

    #[tokio::test]
    async fn orchestrate_single_subtask_success() {
        let cap = AgentCapability::new("PrActor", ["code_review"], "actors.pr.>");
        let plan = plan_with(vec![subtask("1", "code_review")]);
        let caller = MockAgentCaller::returning(b"LGTM".to_vec());
        let engine = engine_with(vec![cap], plan, "All good.", caller).await;

        let result = engine.orchestrate("Review PR #123").await.unwrap();

        assert_eq!(result.sub_results.len(), 1);
        assert!(result.sub_results[0].success);
        assert_eq!(result.sub_results[0].output, b"LGTM");
        assert_eq!(result.synthesis, "All good.");
    }

    #[tokio::test]
    async fn orchestrate_parallel_subtasks() {
        let cap1 = AgentCapability::new("ReviewActor", ["code_review"], "actors.review.>");
        let cap2 = AgentCapability::new("SecurityActor", ["security_analysis"], "actors.sec.>");
        let plan = plan_with(vec![
            subtask("1", "code_review"),
            subtask("2", "security_analysis"),
        ]);
        let caller = MockAgentCaller::returning(b"ok".to_vec());
        let engine =
            engine_with(vec![cap1, cap2], plan, "Both passed.", caller).await;

        let result = engine.orchestrate("Full review").await.unwrap();

        assert_eq!(result.sub_results.len(), 2);
        assert!(result.sub_results.iter().all(|r| r.success));
    }

    #[tokio::test]
    async fn missing_capability_produces_failed_subtask_result() {
        let cap = AgentCapability::new("PrActor", ["code_review"], "actors.pr.>");
        let plan = plan_with(vec![subtask("1", "nonexistent_capability")]);
        let caller = MockAgentCaller::returning(b"ok".to_vec());
        let engine = engine_with(vec![cap], plan, "partial", caller).await;

        let result = engine.orchestrate("task").await.unwrap();

        assert_eq!(result.sub_results.len(), 1);
        assert!(!result.sub_results[0].success);
        assert!(result.sub_results[0].error.as_deref().unwrap().contains("no agent registered"));
    }

    #[tokio::test]
    async fn agent_call_failure_produces_failed_subtask_result() {
        let cap = AgentCapability::new("PrActor", ["code_review"], "actors.pr.>");
        let plan = plan_with(vec![subtask("1", "code_review")]);
        let caller = MockAgentCaller::failing("agent timed out");
        let engine = engine_with(vec![cap], plan, "partial", caller).await;

        let result = engine.orchestrate("task").await.unwrap();

        assert!(!result.sub_results[0].success);
        assert!(result.sub_results[0].error.as_deref().unwrap().contains("agent timed out"));
    }

    #[tokio::test]
    async fn empty_plan_produces_empty_results_with_synthesis() {
        let plan = plan_with(vec![]);
        let caller = MockAgentCaller::returning(b"".to_vec());
        let engine = engine_with(vec![], plan, "nothing to do", caller).await;

        let result = engine.orchestrate("empty task").await.unwrap();

        assert!(result.sub_results.is_empty());
        assert_eq!(result.synthesis, "nothing to do");
    }

    #[tokio::test]
    async fn orchestrate_returns_error_when_registry_fails() {
        // A registry store whose keys() always errors.
        #[derive(Clone)]
        struct FailingStore;

        #[derive(Debug)]
        struct StoreErr;
        impl std::fmt::Display for StoreErr {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "store unavailable")
            }
        }
        impl std::error::Error for StoreErr {}

        impl trogon_registry::RegistryStore for FailingStore {
            type PutError = StoreErr;
            type GetError = StoreErr;
            type DeleteError = StoreErr;
            type KeysError = StoreErr;

            async fn put(&self, _key: &str, _value: bytes::Bytes) -> Result<u64, StoreErr> {
                Err(StoreErr)
            }
            async fn get(&self, _key: &str) -> Result<Option<bytes::Bytes>, StoreErr> {
                Err(StoreErr)
            }
            async fn delete(&self, _key: &str) -> Result<(), StoreErr> {
                Err(StoreErr)
            }
            async fn keys(&self) -> Result<Vec<String>, StoreErr> {
                Err(StoreErr)
            }
        }

        let registry = Registry::new(FailingStore);
        let plan = plan_with(vec![subtask("1", "code_review")]);
        let provider = MockOrchestratorProvider::returning(plan, "ok");
        let caller = MockAgentCaller::returning(b"ok".to_vec());
        let engine = OrchestratorEngine::new(provider, caller, registry);

        let err = engine.orchestrate("task").await.unwrap_err();
        assert!(matches!(err, OrchestratorError::Planning(_)));
        assert!(err.to_string().contains("planning failed"));
    }

    #[tokio::test]
    async fn provider_planning_failure_propagates_as_error() {
        // MockOrchestratorProvider with no plan set always returns Planning error
        let provider = MockOrchestratorProvider { plan: None, synthesis: None };
        let store = MockRegistryStore::new();
        let registry = Registry::new(store);
        let caller = MockAgentCaller::returning(b"".to_vec());
        let engine = OrchestratorEngine::new(provider, caller, registry);

        let err = engine.orchestrate("impossible task").await.unwrap_err();
        assert!(matches!(err, OrchestratorError::Planning(_)));
    }

    #[tokio::test]
    async fn synthesis_failure_propagates_as_error() {
        let cap = AgentCapability::new("PrActor", ["code_review"], "actors.pr.>");
        let plan = plan_with(vec![subtask("1", "code_review")]);
        // synthesis: None → MockOrchestratorProvider returns Synthesis error
        let provider = MockOrchestratorProvider { plan: Some(plan), synthesis: None };
        let store = MockRegistryStore::new();
        let registry = Registry::new(store.clone());
        registry.register(&cap).await.unwrap();
        let caller = MockAgentCaller::returning(b"ok".to_vec());
        let engine = OrchestratorEngine::new(provider, caller, registry);

        let err = engine.orchestrate("task").await.unwrap_err();
        assert!(matches!(err, OrchestratorError::Synthesis(_)));
    }

    #[tokio::test]
    async fn all_subtasks_fail_synthesis_is_still_called() {
        // No registered capabilities → all subtasks fail with "no agent registered"
        // The synthesis step is still reached and produces a result.
        let plan = plan_with(vec![
            subtask("1", "missing_cap_a"),
            subtask("2", "missing_cap_b"),
        ]);
        let caller = MockAgentCaller::returning(b"".to_vec());
        let engine = engine_with(vec![], plan, "degraded synthesis", caller).await;

        let result = engine.orchestrate("task with no agents").await.unwrap();

        assert_eq!(result.sub_results.len(), 2);
        assert!(result.sub_results.iter().all(|r| !r.success));
        assert_eq!(result.synthesis, "degraded synthesis");
    }
}
