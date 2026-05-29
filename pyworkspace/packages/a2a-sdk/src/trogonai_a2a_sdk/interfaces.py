from __future__ import annotations

from dataclasses import dataclass
from typing import Mapping, Optional, Protocol, runtime_checkable

from .types import AgentId, AgentRecord, ExchangeRequest, ExchangeResponse


@runtime_checkable
class SvidSource(Protocol):
    async def current(self) -> str: ...


@runtime_checkable
class SubjectTokenSource(Protocol):
    async def current(self) -> str: ...


@runtime_checkable
class Sts(Protocol):
    async def exchange(self, req: ExchangeRequest) -> ExchangeResponse: ...


@runtime_checkable
class Registry(Protocol):
    async def lookup(self, agent_id: AgentId) -> AgentRecord: ...


@runtime_checkable
class MessageTransport(Protocol):
    async def request(
        self,
        subject: str,
        payload: bytes,
        headers: Mapping[str, str],
    ) -> bytes: ...


@dataclass(frozen=True)
class JwksKey:
    key: str
    kid: Optional[str] = None


@runtime_checkable
class Jwks(Protocol):
    async def decoding_key(self, kid: Optional[str] = None) -> JwksKey: ...
