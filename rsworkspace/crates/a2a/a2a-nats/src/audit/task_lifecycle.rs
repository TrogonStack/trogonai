//! Minimal task lifecycle audit payload for `TaskStatusUpdateEvent` transitions.

use a2a::types::TaskState;

use crate::agent_id::A2aAgentId;

/// Emitted when a streaming task transitions to a new `TaskState` (`message/stream`).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskLifecycleEnvelope {
    pub agent_id: String,
    pub task_id: String,
    pub json_rpc_req_id: Option<String>,
    pub prev_task_state: TaskState,
    pub new_task_state: TaskState,
    pub emitted_at: u64,
}

impl TaskLifecycleEnvelope {
    pub fn new(
        agent_id: &A2aAgentId,
        task_id: impl Into<String>,
        json_rpc_req_id: Option<String>,
        prev_task_state: TaskState,
        new_task_state: TaskState,
        emitted_at: u64,
    ) -> Self {
        Self {
            agent_id: agent_id.as_str().to_owned(),
            task_id: task_id.into(),
            json_rpc_req_id,
            prev_task_state,
            new_task_state,
            emitted_at,
        }
    }
}
