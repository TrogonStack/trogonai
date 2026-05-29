import pytest

from trogonai_a2a_sdk import (
    AgentId,
    Audience,
    Caller,
    ChainLoop,
    ChainTooDeep,
    Forbidden,
    Server,
    ServerConfig,
    encode_jwt_for_testing,
)
from trogonai_a2a_sdk.test_doubles import NoopJwks


class CapturingHandler:
    def __init__(self) -> None:
        self.caller: Caller | None = None
        self.payload: bytes | None = None

    async def handle(self, caller: Caller, payload: bytes) -> bytes:
        self.caller = caller
        self.payload = payload
        return b"ok"


def _entry(agent_id: str, wkl: str, aud: str) -> dict:
    return {"sub": agent_id, "agent_id": agent_id, "wkl": wkl, "aud": aud, "iat": 0}


SELF = AgentId.parse("acme/oncall")
EXPECTED_AUD = str(Audience.for_agent(SELF))


async def test_dispatch_happy_path() -> None:
    handler = CapturingHandler()
    server = Server.build(ServerConfig(agent_id=SELF, jwks=NoopJwks(), handler=handler))
    token = encode_jwt_for_testing(
        {
            "aud": EXPECTED_AUD,
            "act_chain": [
                _entry("acme/source", "spiffe://source", EXPECTED_AUD),
                _entry("acme/middle", "spiffe://middle", EXPECTED_AUD),
            ],
            "purpose": "handoff",
            "session_id": "sess-1",
        }
    )
    reply = await server.dispatch(token, b"hi")
    assert reply == b"ok"
    assert handler.caller is not None
    assert len(handler.caller.chain) == 2
    assert handler.caller.originator.agent_id == "acme/source"
    assert handler.caller.direct.agent_id == "acme/middle"
    assert str(handler.caller.purpose) == "handoff"
    assert handler.caller.session_id == "sess-1"


async def test_dispatch_rejects_aud_mismatch() -> None:
    server = Server.build(
        ServerConfig(agent_id=SELF, jwks=NoopJwks(), handler=CapturingHandler())
    )
    token = encode_jwt_for_testing(
        {
            "aud": "urn:trogon:a2a:agent:acme:other",
            "act_chain": [_entry("acme/source", "spiffe://source", "x")],
        }
    )
    with pytest.raises(Forbidden):
        await server.dispatch(token, b"")


async def test_dispatch_rejects_too_deep_chain() -> None:
    server = Server.build(
        ServerConfig(
            agent_id=SELF,
            jwks=NoopJwks(),
            handler=CapturingHandler(),
            max_chain_depth=2,
        )
    )
    token = encode_jwt_for_testing(
        {
            "aud": EXPECTED_AUD,
            "act_chain": [
                _entry("acme/a", "spiffe://a", EXPECTED_AUD),
                _entry("acme/b", "spiffe://b", EXPECTED_AUD),
                _entry("acme/c", "spiffe://c", EXPECTED_AUD),
            ],
        }
    )
    with pytest.raises(ChainTooDeep):
        await server.dispatch(token, b"")


async def test_dispatch_rejects_chain_loop() -> None:
    server = Server.build(
        ServerConfig(agent_id=SELF, jwks=NoopJwks(), handler=CapturingHandler())
    )
    token = encode_jwt_for_testing(
        {
            "aud": EXPECTED_AUD,
            "act_chain": [
                _entry("acme/a", "spiffe://a", EXPECTED_AUD),
                _entry("acme/b", "spiffe://b", EXPECTED_AUD),
                _entry("acme/a", "spiffe://a", EXPECTED_AUD),
            ],
        }
    )
    with pytest.raises(ChainLoop):
        await server.dispatch(token, b"")
