import pytest

from trogonai_a2a_sdk import (
    AgentId,
    AgentRecord,
    CALLER_JWT_HEADER,
    Client,
    ClientConfig,
    Forbidden,
    LookupFailed,
    Purpose,
    PurposeRequired,
)
from trogonai_a2a_sdk.test_doubles import (
    InMemoryRegistry,
    InMemorySts,
    NoopJwks,
    RecordingTransport,
    StaticSubjectTokenSource,
    StaticSvidSource,
)


def _build(self_id: AgentId, registry: InMemoryRegistry, **overrides):
    sts = overrides.get("sts") or InMemorySts()
    transport = overrides.get("transport") or RecordingTransport('{"ok":true}')
    client = Client.build(
        ClientConfig(
            agent_id=self_id,
            svid_source=StaticSvidSource("svid-jwt"),
            subject_token_source=StaticSubjectTokenSource("bootstrap-jwt"),
            sts=sts,
            registry=registry,
            transport=transport,
            jwks=NoopJwks(),
        )
    )
    return client, sts, transport


async def test_call_happy_path() -> None:
    self_id = AgentId.parse("acme/oncall")
    target = AgentId.parse("acme/planner")
    registry = (
        InMemoryRegistry()
        .set(self_id, AgentRecord(allowed_purposes=["handoff"]))
        .set(
            target,
            AgentRecord(allowed_audiences=["urn:trogon:a2a:agent:acme:planner"]),
        )
    )
    client, sts, transport = _build(self_id, registry)

    reply = await client.call(target, {"q": "hello"}, Purpose("handoff"))

    assert reply == {"ok": True}
    assert len(sts.calls) == 1
    assert sts.calls[0].request.audience == "urn:trogon:a2a:agent:acme:planner"
    assert sts.calls[0].request.agent_id == "acme/oncall"
    assert sts.calls[0].request.purpose == "handoff"
    assert len(transport.calls) == 1
    assert transport.calls[0].subject == "mcp.agent.acme.planner.request"
    assert CALLER_JWT_HEADER in transport.calls[0].headers


async def test_call_auto_fills_single_allowed_purpose() -> None:
    self_id = AgentId.parse("acme/oncall")
    target = AgentId.parse("acme/planner")
    registry = (
        InMemoryRegistry()
        .set(self_id, AgentRecord(allowed_purposes=["handoff"]))
        .set(target, AgentRecord())
    )
    client, sts, _ = _build(self_id, registry)
    await client.call(target, {"q": "hi"}, None)
    assert sts.calls[0].request.purpose == "handoff"


async def test_call_requires_purpose_when_multiple_allowed() -> None:
    self_id = AgentId.parse("acme/oncall")
    target = AgentId.parse("acme/planner")
    registry = (
        InMemoryRegistry()
        .set(self_id, AgentRecord(allowed_purposes=["a", "b"]))
        .set(target, AgentRecord())
    )
    client, _, _ = _build(self_id, registry)
    with pytest.raises(PurposeRequired):
        await client.call(target, {"q": "hi"}, None)


async def test_call_rejects_self_call() -> None:
    self_id = AgentId.parse("acme/oncall")
    registry = InMemoryRegistry().set(self_id, AgentRecord())
    client, _, _ = _build(self_id, registry)
    with pytest.raises(Forbidden):
        await client.call(self_id, {}, None)


async def test_call_surfaces_missing_record_as_lookup_failed() -> None:
    self_id = AgentId.parse("acme/oncall")
    registry = InMemoryRegistry().set(self_id, AgentRecord())
    client, _, _ = _build(self_id, registry)
    with pytest.raises(LookupFailed):
        await client.call(AgentId.parse("acme/unknown"), {}, Purpose("handoff"))
