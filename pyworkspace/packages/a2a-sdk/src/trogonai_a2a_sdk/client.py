from __future__ import annotations

import json
from dataclasses import dataclass
from typing import Any, Literal, Optional

from .errors import (
    ConfigError,
    Forbidden,
    LookupFailed,
    PurposeRequired,
    SdkError,
    SerializationError,
    TransportError,
)
from .interfaces import (
    Jwks,
    MessageTransport,
    Registry,
    Sts,
    SubjectTokenSource,
    SvidSource,
)
from .subject import CALLER_JWT_HEADER, DEFAULT_PURPOSE, agent_request_subject
from .types import AgentId, Audience, ExchangeRequest, Purpose


AudiencePolicy = Literal["strict", "permissive"]


@dataclass
class ClientConfig:
    agent_id: AgentId
    svid_source: SvidSource
    subject_token_source: SubjectTokenSource
    sts: Sts
    registry: Registry
    transport: MessageTransport
    jwks: Optional[Jwks] = None
    audience_policy: AudiencePolicy = "strict"
    max_chain_depth: int = 8


class Client:
    def __init__(self, cfg: ClientConfig) -> None:
        self._cfg = cfg

    @classmethod
    def build(cls, cfg: ClientConfig) -> "Client":
        if cfg.agent_id is None:
            raise ConfigError("agent_id is required")
        if cfg.svid_source is None:
            raise ConfigError("svid_source is required")
        if cfg.subject_token_source is None:
            raise ConfigError("subject_token_source is required")
        if cfg.sts is None:
            raise ConfigError("sts is required")
        if cfg.registry is None:
            raise ConfigError("registry is required")
        if cfg.transport is None:
            raise ConfigError("transport is required")
        return cls(cfg)

    @property
    def agent_id(self) -> AgentId:
        return self._cfg.agent_id

    async def call(
        self,
        target: AgentId,
        payload: Any,
        purpose: Optional[Purpose],
    ) -> Any:
        if target == self._cfg.agent_id:
            raise Forbidden("self-call is not permitted")

        try:
            target_record = await self._cfg.registry.lookup(target)
        except SdkError:
            raise
        except Exception as exc:
            raise LookupFailed(f"registry lookup failed for {target}") from exc

        resolved_purpose = await self._resolve_purpose(purpose)
        audience = (
            target_record.allowed_audiences[0]
            if target_record.allowed_audiences
            else str(Audience.for_agent(target))
        )

        actor_token = await self._cfg.svid_source.current()
        subject_token = await self._cfg.subject_token_source.current()

        scope = (
            " ".join(target_record.allowed_audiences)
            if target_record.allowed_audiences
            else None
        )

        exchange = await self._cfg.sts.exchange(
            ExchangeRequest(
                subject_token=subject_token,
                actor_token=actor_token,
                audience=audience,
                scope=scope,
                purpose=str(resolved_purpose),
                agent_id=str(self._cfg.agent_id),
            )
        )

        try:
            body = json.dumps(payload).encode("utf-8")
        except (TypeError, ValueError) as exc:
            raise SerializationError("failed to encode request payload") from exc

        subject = agent_request_subject(target)
        try:
            reply = await self._cfg.transport.request(
                subject,
                body,
                {CALLER_JWT_HEADER: exchange.access_token},
            )
        except SdkError:
            raise
        except Exception as exc:
            raise TransportError(f"transport request failed: {subject}") from exc

        try:
            return json.loads(reply.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as exc:
            raise SerializationError("failed to decode response payload") from exc

    async def _resolve_purpose(self, purpose: Optional[Purpose]) -> Purpose:
        if purpose is not None:
            return purpose
        own_record = await self._cfg.registry.lookup(self._cfg.agent_id)
        allowed = own_record.allowed_purposes
        if len(allowed) == 0:
            return Purpose(DEFAULT_PURPOSE)
        if len(allowed) == 1:
            return Purpose(allowed[0])
        raise PurposeRequired()
