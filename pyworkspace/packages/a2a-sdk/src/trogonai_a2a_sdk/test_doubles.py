from __future__ import annotations

import time
from dataclasses import dataclass, field
from typing import Mapping, Optional

from .errors import LookupFailed
from .interfaces import (
    Jwks,
    JwksKey,
    MessageTransport,
    Registry,
    Sts,
    SubjectTokenSource,
    SvidSource,
)
from .server import encode_jwt_for_testing
from .types import (
    ActChainEntry,
    AgentId,
    AgentRecord,
    Audience,
    ExchangeRequest,
    ExchangeResponse,
)


class StaticSvidSource(SvidSource):
    def __init__(self, token: str) -> None:
        self._token = token

    async def current(self) -> str:
        return self._token


class StaticSubjectTokenSource(SubjectTokenSource):
    def __init__(self, token: str) -> None:
        self._token = token

    async def current(self) -> str:
        return self._token


class NoopJwks(Jwks):
    async def decoding_key(self, kid: Optional[str] = None) -> JwksKey:
        return JwksKey(key="noop", kid=kid)


class InMemoryRegistry(Registry):
    def __init__(self) -> None:
        self._records: dict[str, AgentRecord] = {}

    def set(self, agent_id: AgentId, record: AgentRecord) -> "InMemoryRegistry":
        self._records[str(agent_id)] = record
        return self

    async def lookup(self, agent_id: AgentId) -> AgentRecord:
        record = self._records.get(str(agent_id))
        if record is None:
            raise LookupFailed(f"no record for {agent_id}")
        return record


@dataclass
class StsCall:
    request: ExchangeRequest
    response: ExchangeResponse


class InMemorySts(Sts):
    def __init__(self) -> None:
        self.calls: list[StsCall] = []
        self._chain: list[ActChainEntry] = []

    def with_chain(self, chain: list[ActChainEntry]) -> "InMemorySts":
        self._chain = chain
        return self

    async def exchange(self, req: ExchangeRequest) -> ExchangeResponse:
        next_chain = list(self._chain) + [
            ActChainEntry(
                sub=req.agent_id or "unknown",
                wkl=f"spiffe://test/{req.agent_id or 'unknown'}",
                aud=req.audience,
                iat=int(time.time()),
                agent_id=req.agent_id,
                purpose=req.purpose,
            )
        ]
        chain_payload = [
            {
                "sub": entry.sub,
                "wkl": entry.wkl,
                "aud": entry.aud,
                "iat": entry.iat,
                "agent_id": entry.agent_id,
                "purpose": entry.purpose,
            }
            for entry in next_chain
        ]
        token = encode_jwt_for_testing(
            {
                "aud": req.audience,
                "act_chain": chain_payload,
                "purpose": req.purpose,
            }
        )
        response = ExchangeResponse(
            access_token=token, expires_in=60, token_type="Bearer"
        )
        self.calls.append(StsCall(request=req, response=response))
        return response


@dataclass
class TransportCall:
    subject: str
    payload: bytes
    headers: dict[str, str] = field(default_factory=dict)


class RecordingTransport(MessageTransport):
    def __init__(self, reply: bytes | str) -> None:
        self.calls: list[TransportCall] = []
        self._reply = reply.encode("utf-8") if isinstance(reply, str) else reply

    def set_reply(self, reply: bytes | str) -> None:
        self._reply = reply.encode("utf-8") if isinstance(reply, str) else reply

    async def request(
        self,
        subject: str,
        payload: bytes,
        headers: Mapping[str, str],
    ) -> bytes:
        self.calls.append(
            TransportCall(subject=subject, payload=payload, headers=dict(headers))
        )
        return self._reply


def expected_audience_for(agent: AgentId) -> str:
    return str(Audience.for_agent(agent))
