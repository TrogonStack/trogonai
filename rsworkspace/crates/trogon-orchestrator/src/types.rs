use serde::{Deserialize, Serialize};

/// One delegated unit of work sent to a sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubTask {
    /// Unique identifier for this sub-task within the plan.
    pub id: String,
    /// The agent capability required to handle this sub-task.
    pub capability: String,
    /// Human-readable description of what is expected.
    pub description: String,
    /// The payload to send to the agent (opaque bytes, serialized by the caller).
    pub payload: Vec<u8>,
}

/// LLM-generated decomposition of a top-level task into parallel sub-tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlan {
    pub subtasks: Vec<SubTask>,
    pub reasoning: String,
}

/// The outcome of one sub-agent invocation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubTaskResult {
    pub subtask_id: String,
    pub capability: String,
    /// Raw bytes returned by the agent.
    pub output: Vec<u8>,
    pub success: bool,
    pub error: Option<String>,
}

impl SubTaskResult {
    pub fn ok(subtask_id: impl Into<String>, capability: impl Into<String>, output: Vec<u8>) -> Self {
        Self {
            subtask_id: subtask_id.into(),
            capability: capability.into(),
            output,
            success: true,
            error: None,
        }
    }

    pub fn err(
        subtask_id: impl Into<String>,
        capability: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        Self {
            subtask_id: subtask_id.into(),
            capability: capability.into(),
            output: vec![],
            success: false,
            error: Some(error.into()),
        }
    }
}

/// The aggregated result of a full orchestration round.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationResult {
    /// The task as originally described.
    pub task: String,
    pub plan: TaskPlan,
    pub sub_results: Vec<SubTaskResult>,
    /// LLM-generated synthesis combining all sub-agent outputs.
    pub synthesis: String,
}

/// Error type for the orchestrator.
#[derive(Debug)]
pub enum OrchestratorError {
    Planning(String),
    Synthesis(String),
    NoAgent(String),
    Serialization(serde_json::Error),
}

impl std::fmt::Display for OrchestratorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrchestratorError::Planning(e) => write!(f, "planning failed: {e}"),
            OrchestratorError::Synthesis(e) => write!(f, "synthesis failed: {e}"),
            OrchestratorError::NoAgent(cap) => write!(f, "no agents available for capability '{cap}'"),
            OrchestratorError::Serialization(e) => write!(f, "serialization error: {e}"),
        }
    }
}

impl std::error::Error for OrchestratorError {}

impl From<serde_json::Error> for OrchestratorError {
    fn from(e: serde_json::Error) -> Self {
        OrchestratorError::Serialization(e)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subtask_result_ok_sets_success_and_output() {
        let r = SubTaskResult::ok("t1", "code_review", b"LGTM".to_vec());
        assert_eq!(r.subtask_id, "t1");
        assert_eq!(r.capability, "code_review");
        assert_eq!(r.output, b"LGTM");
        assert!(r.success);
        assert!(r.error.is_none());
    }

    #[test]
    fn subtask_result_err_sets_failure_and_error() {
        let r = SubTaskResult::err("t2", "security_analysis", "timed out");
        assert_eq!(r.subtask_id, "t2");
        assert!(!r.success);
        assert!(r.output.is_empty());
        assert_eq!(r.error.as_deref(), Some("timed out"));
    }

    #[test]
    fn orchestrator_error_display_planning() {
        let e = OrchestratorError::Planning("bad json".into());
        assert!(e.to_string().contains("planning failed"));
        assert!(e.to_string().contains("bad json"));
    }

    #[test]
    fn orchestrator_error_display_synthesis() {
        let e = OrchestratorError::Synthesis("model overloaded".into());
        assert!(e.to_string().contains("synthesis failed"));
    }

    #[test]
    fn orchestrator_error_display_no_agent() {
        let e = OrchestratorError::NoAgent("code_review".into());
        assert!(e.to_string().contains("no agents available"));
        assert!(e.to_string().contains("code_review"));
    }

    #[test]
    fn orchestrator_error_from_serde() {
        let raw = serde_json::from_str::<serde_json::Value>("invalid").unwrap_err();
        let e = OrchestratorError::from(raw);
        assert!(matches!(e, OrchestratorError::Serialization(_)));
    }

    #[test]
    fn task_plan_serde_round_trip() {
        let plan = TaskPlan {
            subtasks: vec![SubTask {
                id: "1".into(),
                capability: "review".into(),
                description: "Check the diff".into(),
                payload: b"{\"pr\":1}".to_vec(),
            }],
            reasoning: "single reviewer needed".into(),
        };
        let json = serde_json::to_string(&plan).unwrap();
        let restored: TaskPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.subtasks.len(), 1);
        assert_eq!(restored.subtasks[0].capability, "review");
        assert_eq!(restored.reasoning, "single reviewer needed");
    }

    #[test]
    fn orchestration_result_serde_round_trip() {
        let result = OrchestrationResult {
            task: "Review PR #5".into(),
            plan: TaskPlan { subtasks: vec![], reasoning: "none".into() },
            sub_results: vec![SubTaskResult::ok("1", "review", b"ok".to_vec())],
            synthesis: "All good.".into(),
        };
        let json = serde_json::to_string(&result).unwrap();
        let restored: OrchestrationResult = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.task, "Review PR #5");
        assert_eq!(restored.synthesis, "All good.");
        assert_eq!(restored.sub_results.len(), 1);
    }
}
