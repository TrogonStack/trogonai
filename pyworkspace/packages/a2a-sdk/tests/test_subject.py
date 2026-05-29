from trogonai_a2a_sdk import AgentId, agent_queue_group, agent_request_subject


def test_request_subject_shape() -> None:
    aid = AgentId.parse("acme/planner")
    assert agent_request_subject(aid) == "mcp.agent.acme.planner.request"


def test_queue_group_shape() -> None:
    aid = AgentId.parse("acme/planner")
    assert agent_queue_group(aid) == "agent-acme-planner"
