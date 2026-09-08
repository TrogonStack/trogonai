# NetBird Agent Network — notes

> Notes taken 2026-07-08 from `github.com/netbirdio/netbird`: dirs `agent-network/`, `docs/agent-networks/`, and source under `proxy/internal/{middleware,llm}` and `management/internals/modules/{reverseproxy,agentnetwork}`. Grounded in the in-repo design docs + Go source (paths cited), not marketing copy. NetBird is open-source (AGPL core), self-hostable, written in Go. The feature is "beta but running in production" per the README.
>
> Companion doc: `VERCEL_AGENTGATEWAY_RESEARCH.md` (Vercel AI Gateway + agentgateway + helsinki gap analysis). NetBird is the third, network-layer, point of comparison there.

---

## 0. The one-line differentiator

NetBird is the only one of the compared systems that governs LLM traffic at the **network layer**. It is not a standalone gateway product; it is a feature bolted onto NetBird's existing **WireGuard overlay mesh + reverse proxy**. Every agent is a NetBird peer with an IdP-tied identity; the governed LLM endpoint (e.g. `https://mirror.netbird.ai`) is reachable **only over the encrypted tunnel** after IdP auth, never from the public internet.

This is a fundamentally different trust model from Vercel (hosted egress), agentgateway (a data plane you deploy), and helsinki (NATS identity / subject-scoped). Identity comes from the WireGuard peer + IdP, not from a bearer token the caller presents to the gateway.

---

## 1. Architecture: synthesized middleware chain

- **Two reused components, no new service.** The `proxy/` reverse proxy is the data plane (terminates LLM requests over WireGuard). `management/internals/modules/reverseproxy` + `.../agentnetwork` is the control plane. Agent Network is a *synthesizer* that emits per-peer proxy config.
- **Config → runtime flow:**
  1. Operator edits Provider / Policy / Guardrail / BudgetRule / Settings via REST.
  2. Management persists (gorm / SQL).
  3. `network_map.Controller` calls `SynthesizeServices(ctx, store, accountID)` **on every network-map push** (not on a timer; cost is O(peers × policies × providers) per push).
  4. Services + a `MiddlewareConfig` list stream to the proxy over gRPC.
  5. The proxy's `middleware_translate` turns proto configs into a runtime `middleware.Chain`.
  - **Chain replacement is live — no restart, in-flight requests unaffected.**
- **`SynthesizeServices` is the single source of truth** for the wire format the proxy runs. Design doc: "Anything the proxy does that the synthesizer didn't request is a bug." The translate step must reject unknown middleware IDs — silently dropping e.g. `llm_limit_check` would mean unbounded spend.
- **Middleware framework** — generic plugin system:
  - Three slots: `on_request`, `on_response`, `terminal`.
  - Per-instance `fail_open` / `fail_closed`, `timeout_ms`, `can_mutate`.
  - Each middleware declares `MetadataKeys()`; the accumulator drops any KV outside that allowlist.
  - Header/body rewrites go through a gated `Mutations` path. `Authorization` is blocked on the generic header path — auth injection uses a separate trusted `UpstreamRewrite.AuthHeader` field.
  - Body-tap has hard memory bounds: 1 MiB per direction, 256 MiB shared budget; deep-copies body up to 16× per chain (a perf hot-spot).

---

## 2. The 8-middleware LLM chain (canonical order)

Executed per LLM request, in synth-defined order — the order encodes correctness invariants.

| # | Middleware | Slot | Role |
|---|---|---|---|
| 1 | `llm_request_parser` | OnRequest | Detect provider (URL sniff via `DetectParser`, or by name when synth stamps `provider_id`), decode body → `{model, stream}`, extract prompt. Path-routed providers (Vertex/Bedrock) short-circuit: model pulled from the URL path. |
| 2 | `llm_router` | OnRequest | Three-pass route select: filter by `Models` claim → vendor-pin → filter by `AllowedGroupIDs` intersection → model precedence over path → longest-`UpstreamPath`-prefix tie-break. Does upstream rewrite + auth strip/inject. Deny codes `model_not_routable` / `no_authorised_provider`. GCP path can mint short-lived OAuth2 tokens from a `keyfile::` key. |
| 3 | `llm_limit_check` | OnRequest | Pre-flight gRPC `CheckLLMPolicyLimits(provider, model, est_tokens, groups, user)`, 2s timeout, **fail-open** (nil mgmt client / RPC error → allow, so a management outage doesn't kill all LLM traffic). Stamps attribution metadata on allow. |
| 4 | `llm_identity_inject` | OnRequest | Inject NetBird identity (peer email or UserID) + authorising-group tags into upstream headers/body. Two shapes: LiteLLM-style `HeaderPair`, Portkey-style `JSONMetadata`, plus catalog `ExtraHeaders`. Anti-spoof: every `HeadersAdd` is preceded by `HeadersRemove` of the same name so client-supplied identity never reaches upstream. |
| 5 | `llm_guardrail` | OnRequest | Model-allowlist deny (case-insensitive; empty allowlist = disabled) + optional prompt capture. Deny code `model_blocked`. |
| 6 | `llm_response_parser` | OnResponse | Parse usage tokens + completion from JSON, SSE (`text/event-stream`), or AWS binary event-stream (Bedrock). Partial-chunk tolerant. |
| 7 | `cost_meter` | OnResponse | Token buckets → USD via `pricing.Loader` (embedded `defaults_pricing.yaml`, hot-reloadable override, atomic swap). Closed-set skip reasons (`unknown_model`, `zero_tokens`, etc.). Provider-agnostic. |
| 8 | `llm_limit_record` | OnResponse | Post-flight gRPC `RecordLLMUsage(provider, model, prompt_t, completion_t, cost, groups, user)`, 5s, errors swallowed (response already served). |

**Record-once invariant:** `llm_limit_check` must precede `llm_router` (a denied request never hits upstream) and must pair with `llm_limit_record` (a checked request is always recorded, or rate-limit semantics break). The recorder has an independent skip-on-missing-attribution guard so no phantom counters materialise even if the chain is misordered.

---

## 3. Budget rules: "min-wins, all-must-pass"

The core (and most surprising) semantic:

- A budget rule binds `(group set, user set)` to `(window, ceiling)`.
- At check time **every** matching rule is evaluated. If **any** rule has zero remaining quota, the whole request is denied.
- Hard caps that stop requests once the budget is hit.
- The proxy never decides locally — it always asks management (`CheckLLMPolicyLimits`) and reports back (`RecordLLMUsage`), keeping account-wide accounting in one place and avoiding per-proxy drift.
- Dashboard "Budget Dashboard" tab polls `/api/agent-network/consumption` (REST poll, not gRPC/WebSocket).

---

## 4. Provider model, pricing, PII

- **Provider record** (`agent_network_providers` table, per account): `UpstreamURL`, encrypted `APIKey`, `ExtraValues` (operator-typed catalog headers), `Models []{ID, InputPer1k, OutputPer1k}` (operator pins prices; empty = all catalog models at catalog prices), `SkipTLSVerification` (for self-hosted upstreams), a per-provider ed25519 session keypair for OIDC session JWTs, and customizable identity-header names.
  - **BYOK is server-side:** the operator's provider key lives in management, injected by the proxy. Agents never hold provider API keys.
- **Pricing:** per-provider cost formulas matter — OpenAI cached tokens are a **subset** of input; Anthropic `cache_read` / `cache_creation` are **additive**. Loader is symlink-safe (`O_NOFOLLOW`, 1 MiB cap), embedded defaults + mtime hot-reload with atomic pointer swap.
- **PII redaction:** parser-side (`llm_guardrail.RedactPII`, a single exported contract) run **before** metadata is stamped, because the access-log sink reads the raw prompt/completion keys. Regex set: email, SSN, phone (E.164 + NA), bearer tokens, IPv4, credit cards, names. Account-level `RedactPii` toggle.
- **Capture pointers:** prompt/completion capture is a `*bool` with three-state semantics — `nil` = legacy emit (back-compat default), `false` = suppress the key entirely, `true` = emit. Driven by the account `EnablePromptCollection` toggle. A missing pointer must **not** be treated as `false` (that would silently suppress capture for legacy callers).
- **Access logs:** `AccessLogEntry` carries geo, user, auth method, bytes up/down, status, latency, protocol. Gated on account `EnableLogCollection` (off = chain still enforces budgets, but no audit trail is kept). Deny paths stamp `llm_policy.decision=deny` + a matching reason for pivoting.

---

## 5. Providers supported / API surfaces

- **Client-facing API shapes parsed:**
  - OpenAI Chat Completions (`/v1/chat/completions`, also bare `/chat/completions` for Cloudflare AI Gateway), OpenAI Responses (`/v1/responses`), legacy Completions.
  - Anthropic Messages (`/v1/messages`) + legacy `/v1/complete`.
  - AWS Bedrock — InvokeModel + Converse, model in URL path, AWS binary event-stream for streaming.
  - Vertex AI — path-routed; Anthropic publisher metered, Gemini/`google` publisher **denied as unmeterable** (`llm_policy.unmeterable_publisher`, 403) rather than forwarded uncounted.
- **Explicitly designed to sit in front of existing gateways:** LiteLLM, Portkey, Bifrost, Cloudflare AI Gateway. The `provider_id` synth-stamp bypasses URL sniffing so non-canonical gateway URLs still parse. NetBird positions itself as an **identity / governance layer on top of** an AI gateway, not a replacement for one.

---

## 6. What to steal for helsinki

Most of NetBird's LLM features overlap the Category A/B gaps already in `VERCEL_AGENTGATEWAY_RESEARCH.md` Part 4. NetBird-specific angles worth adopting:

- **Network-layer identity binding** — agents authenticated as WireGuard peers via IdP; governed endpoint unreachable off-tunnel. helsinki's analogue is NATS-account identity + auth-callout; NetBird's "endpoint not on the public internet" posture is a strong story.
- **Server-side BYOK with zero agent-held keys**, injected by a reverse proxy → maps directly to Part 5 P0 item 1 (LLM egress gateway).
- **"min-wins all-must-pass" multi-rule budget model** — a concrete, legible budget semantic for Part 5 P0 item 3. Simpler than agentgateway's token-bucket CEL.
- **Live chain replacement on config push** with a synthesizer as single source of truth — a clean control-plane pattern; helsinki could mirror it with NATS config streams driving a per-caller policy chain.
- **Fail-open limit check + record-once pairing** — good operational defaults if we build the LLM gateway.
- **Three-state capture pointer for prompt/PII** (`nil` / `false` / `true`) — a careful default-preserving pattern for privacy toggles.

---

## 7. Where NetBird is thinner than agentgateway / helsinki

- **No MCP or A2A awareness at all** — purely an LLM egress governor. No tool-level RBAC, no MCP federation, no agent-card handling. helsinki and agentgateway both far exceed it here.
- **No advanced routing** — no failover chains / weighted split / cost-latency `sort`; routing is allowlist + first-match by model/group/path.
- **No streaming guardrails, no external moderation connectors** — guardrail = static model allowlist + regex PII only. agentgateway (Bedrock Guardrails, Model Armor, webhook, streaming) is much deeper.
- **Coupled to NetBird** — you must run the WireGuard mesh + NetBird management/proxy to get any of it; not a drop-in library.
- **Rough edges (self-flagged in docs):** string-typed `decision` / `deny_code` on the gRPC contract (needs enum pinning), no OTel export of gateway telemetry noted, reaper/GC of stale synth services cut from scope.

---

## 8. Three-way positioning summary

| Axis | Vercel AI Gateway | agentgateway | NetBird Agent Network |
|---|---|---|---|
| Shape | Hosted SaaS egress | Self-host Rust data plane | Feature on WireGuard mesh + reverse proxy |
| Identity model | API key / OIDC token | JWT/OAuth, virtual keys | WireGuard peer + IdP (network-layer) |
| LLM governance | Deep (routing, spend, ZDR) | Deep (routing, budgets, guardrails) | Focused (budget rules, model allowlist, cost) |
| MCP / A2A | MCP via AI SDK | Deepest MCP + A2A proxy | None |
| Guardrails | None built-in | Broadest (moderation, PII, webhook, streaming) | Regex PII + model allowlist only |
| BYOK | Yes | Yes | Yes, server-side, agents hold no keys |
| Network posture | Public egress endpoint | Wherever you deploy | Governed endpoint off public internet |

---

## Sources

- `agent-network/README.md`
- `docs/agent-networks/00-overview.md`, `01-end-to-end-flows.md`
- `docs/agent-networks/modules/31-proxy-middleware-builtin.md`, `32-proxy-llm-parsers.md`, `21-management-agentnetwork.md`
- `proxy/internal/middleware/**`, `proxy/internal/llm/**`
- `management/internals/modules/reverseproxy/{domain,proxy,service,accesslogs}/*.go`
- `management/internals/modules/agentnetwork/types/provider.go`
