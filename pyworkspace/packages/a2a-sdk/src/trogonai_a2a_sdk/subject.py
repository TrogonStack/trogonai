from .types import AgentId


CALLER_JWT_HEADER = "A2a-Caller-Jwt"
DEFAULT_PURPOSE = "default"
MAX_ACT_CHAIN_DEPTH = 8


def agent_request_subject(agent: AgentId) -> str:
    return f"mcp.agent.{agent.tenant}.{agent.name}.request"


def agent_queue_group(agent: AgentId) -> str:
    return f"agent-{agent.tenant}-{agent.name}"
