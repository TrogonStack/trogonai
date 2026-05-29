# @trogonai/a2a-sdk

TypeScript port of the [`trogon-a2a-sdk`](../../../rsworkspace/crates/trogon-a2a-sdk) Rust crate. Same contract: configure **who you are** and **what you want to do**, the SDK owns registry lookup, STS token exchange, mesh-token attachment, and inbound verification.

> **Status:** Skeleton — mock-based; real NATS transport, JWKS over HTTP/KV, JWS signature verification, and OpenTelemetry spans land in a follow-up. Today the SDK is wired against in-memory doubles so the contract shape and pipeline can be exercised before transport ships.

Contract: [`docs/identity/sdk.md`](../../../docs/identity/sdk.md).

## Quick start

```ts
import {
  AgentId,
  Client,
  Purpose,
} from "@trogonai/a2a-sdk";
import {
  InMemoryRegistry,
  InMemorySts,
  NoopJwks,
  RecordingTransport,
  StaticSubjectTokenSource,
  StaticSvidSource,
} from "@trogonai/a2a-sdk/src/test-doubles.js";

const self = AgentId.parse("acme/oncall");
const planner = AgentId.parse("acme/planner");

const registry = new InMemoryRegistry()
  .set(self, { allowedAudiences: [], allowedPurposes: ["handoff"] })
  .set(planner, {
    allowedAudiences: ["urn:trogon:a2a:agent:acme:planner"],
    allowedPurposes: [],
  });

const client = Client.build({
  agentId: self,
  svidSource: new StaticSvidSource("svid-jwt"),
  subjectTokenSource: new StaticSubjectTokenSource("bootstrap-jwt"),
  sts: new InMemorySts(),
  registry,
  transport: new RecordingTransport('{"ok":true}'),
  jwks: new NoopJwks(),
});

const reply = await client.call(planner, { q: "hello" }, new Purpose("handoff"));
```

## Recipe: register and call a new agent

1. **Register both agents** in the in-memory `Registry` (real backing store lands with the NATS transport).
2. **Build the callee** with `Server.build({ agentId, jwks, handler })` and drive it via `server.dispatch(rawJwt, payload)` from your test harness.
3. **Build the caller** with `Client.build({ ... })` wired to shared test doubles.
4. **Call** with an explicit `Purpose` when the caller's registry record has multiple `allowedPurposes`. Omit (`null`) only when the registry lists zero or one allowed purpose; the SDK fills in `default` or the single allowed value.

## Recipe: verify `act_chain` in a handler

After `server.dispatch()` decodes the mesh token, the handler receives a typed `Caller`:

```ts
import type { Caller, Handler } from "@trogonai/a2a-sdk";
import { callerChainDepth, Forbidden } from "@trogonai/a2a-sdk";

class StrictHandler implements Handler {
  async handle(caller: Caller, payload: Uint8Array): Promise<Uint8Array> {
    if (callerChainDepth(caller) > 4) {
      throw new Forbidden("chain too long for this handler");
    }
    return payload;
  }
}
```

## Errors

All thrown values are subclasses of `SdkError` with a stable `code` discriminator. See `src/errors.ts` for the full list: `InvalidAgentId`, `LookupFailed`, `ExchangeFailed`, `Forbidden`, `InvalidToken`, `ChainTooDeep`, `ChainLoop`, `TransportError`, `SerializationError`, `ConfigError`, `PurposeRequired`.

## Roadmap

Intentionally stubbed in this skeleton:

- Real NATS transport (`MessageTransport` over `nats.js`), subscriber wiring, queue groups.
- JWKS fetch over HTTP / NATS KV, with caching and rotation.
- JWS signature verification (likely `jose`). `Server.dispatch` currently decodes claims without verifying signatures — call sites that pass a token MUST verify it themselves until this lands.
- OpenTelemetry span emission matching the Rust SDK's `a2a.call.*` / `a2a.serve.dispatch` shape.
- A `Bridge`-style HTTPS ingress for non-Node runtimes.

## Layout

```
packages/a2a-sdk/
├── src/
│   ├── client.ts         # Client.build + call pipeline
│   ├── server.ts         # Server.build + dispatch (skeleton verify)
│   ├── interfaces.ts     # SvidSource, Sts, Registry, MessageTransport, Jwks
│   ├── types.ts          # AgentId, Audience, Purpose, Caller, ...
│   ├── subject.ts        # subject + constant helpers
│   ├── errors.ts         # SdkError hierarchy
│   ├── test-doubles.ts   # in-memory implementations of every interface
│   └── index.ts          # public surface
└── test/                 # vitest suite
```

## Run

From `tsworkspace/`:

```bash
mise exec -- pnpm install
mise exec -- pnpm --filter @trogonai/a2a-sdk test
mise exec -- pnpm --filter @trogonai/a2a-sdk typecheck
```
