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
