from __future__ import annotations

import base64
import json
from dataclasses import dataclass
from typing import Any, Mapping, Optional, Protocol

from .errors import (
    ChainLoop,
    ChainTooDeep,
    ConfigError,
    Forbidden,
    InvalidToken,
)
from .interfaces import Jwks
from .subject import MAX_ACT_CHAIN_DEPTH
from .types import ActChainEntry, AgentId, Audience, Caller, Purpose


class Handler(Protocol):
    async def handle(self, caller: Caller, payload: bytes) -> bytes: ...


@dataclass
class ServerConfig:
    agent_id: AgentId
    jwks: Jwks
    handler: Handler
    max_chain_depth: int = MAX_ACT_CHAIN_DEPTH


class Server:
    def __init__(self, cfg: ServerConfig) -> None:
        self._cfg = cfg

    @classmethod
    def build(cls, cfg: ServerConfig) -> "Server":
        if cfg.agent_id is None:
            raise ConfigError("agent_id is required")
        if cfg.jwks is None:
            raise ConfigError("jwks is required")
        if cfg.handler is None:
            raise ConfigError("handler is required")
        return cls(cfg)

    # STUB: skeleton dispatch — does NOT verify the JWT signature.
    # The real verify+subscribe path lands with the NATS/JOSE follow-up
    # (PENDING_TODO line 156 umbrella). For now the caller hands us a
    # pre-trusted token; we only enforce structural and aud / chain rules.
    async def dispatch(self, raw_jwt: str, payload: bytes) -> bytes:
        claims, kid = _decode_jwt_claims(raw_jwt)
        expected_aud = str(Audience.for_agent(self._cfg.agent_id))
        token_aud = claims.get("aud")
        if token_aud != expected_aud:
            raise Forbidden(
                f"aud_mismatch: expected {expected_aud}, got {token_aud or '<missing>'}"
            )

        # Touch jwks so the skeleton still exercises the injection point.
        await self._cfg.jwks.decoding_key(kid)

        chain_raw = claims.get("act_chain") or []
        if len(chain_raw) > self._cfg.max_chain_depth:
            raise ChainTooDeep(self._cfg.max_chain_depth, len(chain_raw))
        chain = [_chain_entry(e) for e in chain_raw]
        _detect_chain_loop(chain)
        if not chain:
            raise InvalidToken("empty act_chain")

        purpose_str = claims.get("purpose")
        caller = Caller(
            originator=chain[0],
            chain=chain,
            direct=chain[-1],
            purpose=Purpose(purpose_str) if purpose_str else None,
            session_id=claims.get("session_id"),
        )

        return await self._cfg.handler.handle(caller, payload)


def _decode_jwt_claims(token: str) -> tuple[Mapping[str, Any], Optional[str]]:
    parts = token.split(".")
    if len(parts) != 3:
        raise InvalidToken("JWT must have three dot-separated segments")
    try:
        header = json.loads(_b64url_decode(parts[0]))
    except (ValueError, UnicodeDecodeError) as exc:
        raise InvalidToken("failed to decode JWT header") from exc
    try:
        payload = json.loads(_b64url_decode(parts[1]))
    except (ValueError, UnicodeDecodeError) as exc:
        raise InvalidToken("failed to decode JWT payload") from exc
    if not isinstance(payload, dict):
        raise InvalidToken("JWT payload must be a JSON object")
    kid = header.get("kid") if isinstance(header, dict) else None
    return payload, kid


def _b64url_decode(segment: str) -> str:
    pad = "=" * (-len(segment) % 4)
    return base64.urlsafe_b64decode((segment + pad).encode("ascii")).decode("utf-8")


def _chain_entry(raw: Any) -> ActChainEntry:
    if not isinstance(raw, dict):
        raise InvalidToken("act_chain entry must be an object")
    return ActChainEntry(
        sub=str(raw.get("sub", "")),
        wkl=raw.get("wkl"),
        aud=raw.get("aud"),
        iat=raw.get("iat"),
        agent_id=raw.get("agent_id"),
        purpose=raw.get("purpose"),
    )


def _detect_chain_loop(chain: list[ActChainEntry]) -> None:
    seen: set[tuple[str, str]] = set()
    for entry in chain:
        agent_id = entry.agent_id or ""
        wkl = entry.wkl or ""
        if not agent_id and not wkl:
            continue
        key = (agent_id, wkl)
        if key in seen:
            raise ChainLoop(agent_id, wkl)
        seen.add(key)


def encode_jwt_for_testing(claims: Mapping[str, Any], kid: Optional[str] = None) -> str:
    header: dict[str, Any] = {"alg": "none"}
    if kid:
        header["kid"] = kid

    def _enc(obj: Any) -> str:
        raw = json.dumps(obj).encode("utf-8")
        return base64.urlsafe_b64encode(raw).rstrip(b"=").decode("ascii")

    return f"{_enc(header)}.{_enc(claims)}."
