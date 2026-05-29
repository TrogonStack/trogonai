# trogonai-a2a-sdk

Python port of the [`trogon-a2a-sdk`](../../../rsworkspace/crates/trogon-a2a-sdk) Rust crate and the [`@trogonai/a2a-sdk`](../../../tsworkspace/packages/a2a-sdk) TypeScript package. Same contract: configure **who you are** and **what you want to do**, the SDK owns registry lookup, STS token exchange, mesh-token attachment, and inbound verification.

> **Status:** Skeleton — mock-based; real NATS transport, JWKS over HTTP/KV, JWS signature verification, and OpenTelemetry spans land in a follow-up. Today the SDK is wired against in-memory doubles so the contract shape and pipeline can be exercised before transport ships.

Contract: [`docs/identity/sdk.md`](../../../docs/identity/sdk.md).

## Quick start

```python
from trogonai_a2a_sdk import (
    AgentId,
    AgentRecord,
    Client,
    ClientConfig,
    Purpose,
)
from trogonai_a2a_sdk.test_doubles import (
    InMemoryRegistry,
    InMemorySts,
    NoopJwks,
    RecordingTransport,
    StaticSubjectTokenSource,
    StaticSvidSource,
)

self_id = AgentId.parse("acme/oncall")
planner = AgentId.parse("acme/planner")

registry = (
    InMemoryRegistry()
    .set(self_id, AgentRecord(allowed_purposes=["handoff"]))
    .set(planner, AgentRecord(allowed_audiences=["urn:trogon:a2a:agent:acme:planner"]))
)

client = Client.build(
    ClientConfig(
        agent_id=self_id,
        svid_source=StaticSvidSource("svid-jwt"),
        subject_token_source=StaticSubjectTokenSource("bootstrap-jwt"),
        sts=InMemorySts(),
        registry=registry,
        transport=RecordingTransport('{"ok": true}'),
        jwks=NoopJwks(),
    )
)

reply = await client.call(planner, {"q": "hello"}, Purpose("handoff"))
```

## Recipe: register and call a new agent

1. **Register both agents** in the in-memory `Registry` (real backing store lands with the NATS transport).
2. **Build the callee** with `Server.build(ServerConfig(agent_id, jwks, handler))` and drive it via `server.dispatch(raw_jwt, payload)` from your test harness.
3. **Build the caller** with `Client.build(ClientConfig(...))` wired to shared test doubles.
4. **Call** with an explicit `Purpose` when the caller's registry record has multiple `allowed_purposes`. Pass `None` only when the registry lists zero or one allowed purpose; the SDK fills in `"default"` or the single allowed value.

## Recipe: verify `act_chain` in a handler

After `server.dispatch()` decodes the mesh token, the handler receives a typed `Caller`:

```python
from trogonai_a2a_sdk import Caller, Forbidden, caller_chain_depth


class StrictHandler:
    async def handle(self, caller: Caller, payload: bytes) -> bytes:
        if caller_chain_depth(caller) > 4:
            raise Forbidden("chain too long for this handler")
        return payload
```

## Errors

All raised exceptions are subclasses of `SdkError` with a stable `code` discriminator. See `errors.py` for the full list: `InvalidAgentId`, `LookupFailed`, `ExchangeFailed`, `Forbidden`, `InvalidToken`, `ChainTooDeep`, `ChainLoop`, `TransportError`, `SerializationError`, `ConfigError`, `PurposeRequired`.

## Roadmap

Intentionally stubbed in this skeleton; tracked under `PENDING_TODO.md` line 156:

- Real NATS transport (`MessageTransport` over `nats-py`), subscriber wiring, queue groups.
- JWKS fetch over HTTP / NATS KV, with caching and rotation.
- JWS signature verification (likely `python-jose` or `joserfc`). `Server.dispatch` currently decodes claims without verifying signatures — call sites that pass a token MUST verify it themselves until this lands.
- OpenTelemetry span emission matching the Rust SDK's `a2a.call.*` / `a2a.serve.dispatch` shape.

## Run

From `pyworkspace/`:

```bash
mise exec -- uv sync
mise exec -- uv run --package trogonai-a2a-sdk pytest packages/a2a-sdk/tests
```
