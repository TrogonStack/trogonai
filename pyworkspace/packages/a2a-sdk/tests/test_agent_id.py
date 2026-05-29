import pytest

from trogonai_a2a_sdk import AgentId, InvalidAgentId


def test_parse_splits_tenant_and_name() -> None:
    aid = AgentId.parse("acme/oncall")
    assert aid.tenant == "acme"
    assert aid.name == "oncall"
    assert str(aid) == "acme/oncall"


def test_parse_rejects_missing_slash() -> None:
    with pytest.raises(InvalidAgentId):
        AgentId.parse("acme-oncall")


def test_parse_rejects_empty_tenant() -> None:
    with pytest.raises(InvalidAgentId):
        AgentId.parse("/oncall")


def test_parse_rejects_empty_name() -> None:
    with pytest.raises(InvalidAgentId):
        AgentId.parse("acme/")


def test_parse_rejects_multi_slash() -> None:
    with pytest.raises(InvalidAgentId):
        AgentId.parse("a/b/c")


def test_equality_and_hash() -> None:
    a = AgentId.parse("acme/x")
    b = AgentId.parse("acme/x")
    c = AgentId.parse("acme/y")
    assert a == b
    assert hash(a) == hash(b)
    assert a != c
