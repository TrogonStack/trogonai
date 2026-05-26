# Tier-3 authoritative redaction (gateway)

Authoritative WASM redaction on the gateway ingress path, after Tier-1 SpiceDB and Tier-2 CEL. Skills rewrite JSON subtrees declared in per-skill manifests before the gateway forwards to `{prefix}.agent.{agent_id}.*`.

## Dispatch order

```text
deadline guard → Tier-1 SpiceDB → Tier-2 CEL → Tier-3 redaction → forward
```

## Environment

| Variable | Default | Meaning |
|----------|---------|---------|
| `A2A_GATEWAY_TIER3_REDACTION_ENABLED` | off | Truthy enables `RealTier3RedactionGate` when a policy bundle substrate is loaded |
| `A2A_GATEWAY_POLICY_BUNDLE_DIR` | — | Bundle root; required parent for `{skill}.wasm`, `{skill}.manifest.json`, and optional `{skill}.sig` |
| `A2A_GATEWAY_POLICY_SKILLS` | — | Comma-separated skill slugs to preload WASM + manifests |
| `A2A_GATEWAY_TIER3_SIGNING_PUBKEY` | — | Hex-encoded 32-byte ed25519 public key (no `0x` prefix); when set, preload verifies `{skill}.sig` for every listed skill |

When redaction is off, `NoopTier3RedactionGate` returns `Allow { rewrites: [] }` without invoking WASM.

When `A2A_GATEWAY_TIER3_SIGNING_PUBKEY` is unset, bundle preload skips signature verification (a one-time startup warning is emitted). When set, missing or invalid `{skill}.sig` files cause that skill to fail preload (closed-fail for the skill bundle).

## Signed bundle envelope (`{skill}.sig`)

Each skill may ship a detached signature envelope next to its WASM and manifest:

```json
{
  "version": 1,
  "skill_id": "pii-email",
  "manifest_sha256": "<hex(32)>",
  "wasm_sha256": "<hex(32)>",
  "signature": "<hex(64)>"
}
```

| Field | Meaning |
|-------|---------|
| `version` | Envelope schema version (`1`) |
| `skill_id` | Must match the preload slug / filename stem |
| `manifest_sha256` | SHA-256 of `{skill}.manifest.json` bytes |
| `wasm_sha256` | SHA-256 of `{skill}.wasm` bytes |
| `signature` | ed25519 signature over the 64-byte concatenation `manifest_sha256 \|\| wasm_sha256` (raw digest bytes, not hex) |

Operators sign bundles with the `a2a-sign-bundle` CLI (`a2a-redaction` crate) before deployment:

```bash
a2a-sign-bundle --key <hex-signing-seed> --skill-dir /path/to/bundles
```

Writes `{skill}.sig` for each `*.wasm` in the skill directory (expects a matching `{skill}.manifest.json`).

## Manifest schema

Each preloaded skill may ship `{bundle_dir}/{skill_id}.manifest.json`:

```json
{
  "skill_id": "pii-email",
  "json_path": "$.params.message.parts[0].text",
  "kind": "Masked"
}
```

| Field | Required | Meaning |
|-------|----------|---------|
| `skill_id` | no | Must match the slug / filename stem; defaults to the preload slug |
| `json_path` | **yes** | JSON Pointer (`/params/...`) or `$.`-prefixed path with optional `[index]` segments |
| `kind` | no | Audit rewrite kind: `Replaced`, `Removed`, or `Masked` (default `Replaced`) |

Missing manifest or unreadable `json_path` → skill skipped with a warning (request continues).

## WASM ABI

Guest export: `redact_part(in_ptr, in_len) -> (out_ptr, out_len)` over exported linear `memory` (see `a2a-redaction`).

Host reads UTF-8 JSON at the manifest path, passes bytes to the skill module, and writes the returned JSON back into the request envelope.

### Refusal sentinel

If guest output begins with the UTF-8 prefix:

```text
A2A_T3_REFUSE
```

the gateway treats the invocation as **Refuse** (no forward). Optional reason tag:

```text
A2A_T3_REFUSE:UnauthorizedDataCategory
```

Maps to `Tier3RefusalReason` variants: `SkillPolicyDeniedPart` (default), `InvalidPayloadShape`, `UnauthorizedDataCategory`.

Collisions with legitimate JSON payloads are bundle bugs; the gateway logs a warning when the sentinel appears on non-JSON-shaped output.

## JSON-RPC error codes

| Code | When | Forward? |
|------|------|----------|
| `-32802` | Tier-3 skill **Refuse** | No — `error.data.rule` = skill id |
| `-32801` | Tier-3 **Error** (trap, ABI, invalid guest JSON) — closed-fail | No |
| `-32801` | Tier-1 / Tier-2 policy denial (unchanged) | No |

## Audit envelope

On **Allow** with rewrites, `rewrites` includes Display-formatted entries:

```text
{skill_id}:{Kind}@{json_path}
```

plus the existing ingress→agent route rewrite on forward.

On **Refuse**, audit sets `refusal_skill` to the refusing skill id and populates `rewrites` with any prior skill rewrites from the same evaluation.

On **Error**, `rules_fired` includes `gateway.tier3.engine_error`.

## Telemetry (`tracing`)

| Outcome | Level | Fields |
|---------|-------|--------|
| Allow with rewrites | `info!` | `count`, `caller_id`, `method` |
| Refuse | `warn!` | `skill_id`, `caller_id`, `method`, `reason` |
| Engine error | `error!` | `skill_id`, `caller_id`, `method`, `kind` |

## Implementation map

- Gate + value objects: `a2a-gateway/src/policy/tier3_redaction/`
- Runtime wiring: `a2a-gateway/src/runtime.rs` (`GatewayPolicyStack`, post–Tier-2 call site)
- WASM host + sentinel: `a2a-redaction` (`WasmRedactorHost::preload_skill_bundle`, `TIER3_REFUSE_SENTINEL`)
- Signed bundle verification: `a2a-redaction/src/signed_bundle/` (`verify_signed_bundle`, env-gated via `A2A_GATEWAY_TIER3_SIGNING_PUBKEY`)
- Ingress replies: `a2a-nats::ingress_gateway_tier3_refused_response_bytes`
