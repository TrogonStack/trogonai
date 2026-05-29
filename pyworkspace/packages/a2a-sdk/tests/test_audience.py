from trogonai_a2a_sdk import AgentId, Audience


def test_for_agent_shape() -> None:
    aud = Audience.for_agent(AgentId.parse("acme/planner"))
    assert str(aud) == "urn:trogon:a2a:agent:acme:planner"
