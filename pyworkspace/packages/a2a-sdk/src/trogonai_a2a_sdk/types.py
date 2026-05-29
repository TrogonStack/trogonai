from __future__ import annotations

from dataclasses import dataclass, field
from typing import Optional

from .errors import InvalidAgentId


class AgentId:
    __slots__ = ("_tenant", "_name")

    def __init__(self, tenant: str, name: str) -> None:
        self._tenant = tenant
        self._name = name

    @classmethod
    def parse(cls, value: str) -> "AgentId":
        if "/" not in value:
            raise InvalidAgentId("agent id must be {tenant}/{name}")
        tenant, _, name = value.partition("/")
        if not tenant or not name:
            raise InvalidAgentId("agent id segments must be non-empty")
        if "/" in name:
            raise InvalidAgentId("agent id must be {tenant}/{name}")
        return cls(tenant, name)

    @property
    def tenant(self) -> str:
        return self._tenant

    @property
    def name(self) -> str:
        return self._name

    def __str__(self) -> str:
        return f"{self._tenant}/{self._name}"

    def __repr__(self) -> str:
        return f"AgentId({self!s})"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, AgentId):
            return NotImplemented
        return self._tenant == other._tenant and self._name == other._name

    def __hash__(self) -> int:
        return hash((self._tenant, self._name))


class Audience:
    __slots__ = ("_value",)

    def __init__(self, value: str) -> None:
        self._value = value

    @classmethod
    def for_agent(cls, agent: AgentId) -> "Audience":
        return cls(f"urn:trogon:a2a:agent:{agent.tenant}:{agent.name}")

    def __str__(self) -> str:
        return self._value

    def __repr__(self) -> str:
        return f"Audience({self._value!r})"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Audience):
            return NotImplemented
        return self._value == other._value

    def __hash__(self) -> int:
        return hash(self._value)


class Purpose:
    __slots__ = ("_value",)

    def __init__(self, value: str) -> None:
        self._value = value

    def __str__(self) -> str:
        return self._value

    def __repr__(self) -> str:
        return f"Purpose({self._value!r})"

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Purpose):
            return NotImplemented
        return self._value == other._value

    def __hash__(self) -> int:
        return hash(self._value)


@dataclass(frozen=True)
class ActChainEntry:
    sub: str
    wkl: Optional[str] = None
    aud: Optional[str] = None
    iat: Optional[int] = None
    agent_id: Optional[str] = None
    purpose: Optional[str] = None


@dataclass
class Caller:
    originator: ActChainEntry
    chain: list[ActChainEntry]
    direct: ActChainEntry
    purpose: Optional[Purpose] = None
    session_id: Optional[str] = None


def caller_chain_depth(caller: Caller) -> int:
    return len(caller.chain)


@dataclass
class AgentRecord:
    allowed_audiences: list[str] = field(default_factory=list)
    allowed_purposes: list[str] = field(default_factory=list)
    mesh_token_ttl_s: Optional[int] = None


@dataclass
class ExchangeRequest:
    subject_token: str
    actor_token: str
    audience: str
    scope: Optional[str] = None
    purpose: Optional[str] = None
    agent_id: Optional[str] = None


@dataclass
class ExchangeResponse:
    access_token: str
    expires_in: Optional[int] = None
    token_type: Optional[str] = None
