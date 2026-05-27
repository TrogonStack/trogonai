use crate::types::AgentId;

pub fn agent_request_subject(agent: &AgentId) -> String {
    format!("mcp.agent.{}.{}.request", agent.tenant(), agent.name())
}

pub fn agent_queue_group(agent: &AgentId) -> String {
    format!("agent-{}-{}", agent.tenant(), agent.name())
}
