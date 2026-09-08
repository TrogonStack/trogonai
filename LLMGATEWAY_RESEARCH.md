# LLM Gateway (theopenco/llmgateway) — research notes

> Research date: 2026-08-27. Grounded in the repo's own docs (`apps/docs/content/**`) and source (`apps/gateway`, `apps/api`, `packages/{models,db,shared,actions}`, `ee/**`), read via the GitHub API. File paths and exact identifiers are cited inline so claims can be re-verified. Feature state reflects `main` on the research date; the project ships fast, so re-check specifics before acting.
>
> Companion docs in this repo: `VERCEL_AGENTGATEWAY_RESEARCH.md` (Vercel AI Gateway + agentgateway + helsinki gap analysis) and `NETBIRD_AGENT_NETWORK_NOTES.md`. LLM Gateway is a fourth comparison point and, of the four, the closest open-source analog to **Vercel AI Gateway**.

---

## 0. One-paragraph summary

LLM Gateway (`llmgateway.io`, repo `theopenco/llmgateway`, ~1.6k stars) is an **open-source, self-hostable LLM egress gateway** written in TypeScript. It presents OpenAI-compatible **and** Anthropic-compatible **and** Vercel-AI-SDK-protocol endpoints over ~53 providers and 200+ models, with a genuinely sophisticated **weighted-score routing engine** (price/uptime/throughput/latency/cache factors, epsilon-greedy exploration, sticky sessions, cross-provider failover), full cost/token accounting, gateway + provider prompt caching, multi-scope rate/spend/usage limits, IAM rules, and a large modality surface (chat, embeddings, image/video/speech gen, transcription, OCR, rerank, moderations, realtime, web search). Core is **AGPLv3**; a narrow **commercial `ee/`** tier adds guardrails, audit-log *reads*, per-project routing config, dynamic routes, compliance-based routing, master keys, unlimited retention, and a white-label multi-org admin console. It also ships a resale business model (**embeddable payments**: end-user wallets, your markup as margin). Compared to helsinki it is a pure LLM-egress product with **no MCP-transport / A2A / ACP protocol surface** in the agent-coordination sense.

---

## 1. Architecture & deployment

- **Monorepo** (pnpm workspaces + turbo). Services (all stateless except the datastores):
  - `apps/gateway` — the LLM data plane (Hono). Terminates requests, routes, meters, logs.
  - `apps/api` — management/control API (Hono): orgs, projects, keys, billing, SSO/SCIM, guardrail/audit read routes.
  - `apps/ui` — Next.js dashboard. `apps/admin` — internal/white-label multi-org console (`ee/admin`). `apps/docs`, `apps/playground` (consumer chat "Lounge"), `apps/code` (Dev Plans), `apps/worker` (background rollups/retention).
  - `packages/{db (Drizzle), models, shared, actions}`.
- **Data stores**: PostgreSQL (source of truth) + Redis (response cache, rate-limit sliding windows, routing sticky state, worker queue). Provider API keys injected as secrets.
- **Deploy paths**: single Docker unified image (ports 3002/3003/3005/3006/4001/4002), Docker Compose, or **Helm/Kubernetes** (OCI chart `oci://ghcr.io/theopenco/charts/llmgateway`; gateway scales horizontally with no coordination — recommended production path). Cloud guides for EKS+RDS+ElastiCache, GKE+CloudSQL+Memorystore, AKS. Managed Postgres/Redis recommended in prod.
- **Secrets**: `AUTH_SECRET`, `GATEWAY_API_KEY_HASH_SECRET` (HMAC secret for key fingerprints), `DATABASE_URL`, `REDIS_URL`, `LLM_<PROVIDER>_API_KEY` (comma-separated for load balancing, optional `__ENTERPRISE`/`__PLANS` suffix overrides).
- **Enterprise license**: signed Ed25519 JWT (`packages/shared/src/enterprise-license.ts`), issuer `https://llmgateway.io`, audience `llmgateway-enterprise`. Verified **offline** by api/gateway pods using an embedded public key. Kinds: `enterprise` (org-scoped, needs `organizationId` claim) and `white_label` (deployment-scoped, unlocks the multi-org admin app). Expiry has a **7-day grace** then EE locks (core gateway keeps running). `NODE_ENV !== production` ⇒ always `development` state with EE enabled (matches the license's free dev/test clause).

---

## 2. API surfaces & SDK compatibility

- **OpenAI-compatible** `/v1/chat/completions` is the primary surface; drop-in by swapping base URL to `https://api.llmgateway.io` and `Authorization: Bearer llmgtwy_...`.
- **Anthropic-compatible** `/v1/messages` (`features/anthropic-endpoint.mdx`): transforms Anthropic↔OpenAI internally. Purpose-built for **Claude Code** (`ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN` + optional `ANTHROPIC_MODEL` = any catalog model, e.g. `gpt-5`). Forwards Anthropic `cache_control` (incl. `ttl` 5m/1h), supports server-side web search and Anthropic **tool search** (`tool_search_tool_regex_20251119`, `defer_loading`) where the upstream supports it (dropped on Bedrock, which the gateway routes via Converse).
- **Vercel AI SDK gateway protocol** (`developers/ai-sdk-gateway-protocol.mdx`): implements `@ai-sdk/gateway`'s wire protocol so apps using bare model strings via the AI SDK default provider work unmodified — just repoint `baseURL`. Version-pinned base URLs: AI SDK 5 → `/v1/ai`, SDK 6 → `/v3/ai`, SDK 7 → `/v4/ai`. `getAvailableModels()` and `getCredits()` supported. Gateway-only options live under `providerOptions.llmgateway`.
- **OpenAI Responses API** `/v1/responses` (with `previous_response_id` chaining, 30-day stored-response retention regardless of policy, `store:false` opt-out).
- **MCP server** (`developers/mcp.mdx`): `https://api.llmgateway.io/mcp`, HTTP Streamable + SSE, protocol `2024-11-05`, OAuth 2.0 (`/oauth/{authorize,token,register}`, auth-code + client-credentials). Tools: `chat`, `generate-image`, `generate-nano-banana` (Gemini 3 Pro Image), `list-models`, `list-image-models`. **Note:** this MCP server *exposes LLM Gateway's own inference tools to MCP clients* (Claude Code/Codex/Cursor) — it is **not** an MCP-proxy/federation data plane like agentgateway or helsinki's `mcp-nats`.
- **CLI** `@llmgateway/cli` (alias `lg`): scaffolding templates, `launch`/`configure` for Claude Code / OpenCode / Codex, key & budget & usage management, model directory.

---

## 3. Modalities (unusually broad)

From `apps/docs/content/features/*`. Every one is exposed through an OpenAI-compatible surface unless noted:

| Modality | Notes |
|---|---|
| Chat completions | OpenAI + Anthropic shapes |
| Embeddings | OpenAI-compatible |
| Image generation | images API or chat-completions; also via MCP |
| Video generation | **async API with signed completion callbacks** |
| Speech (TTS) | ElevenLabs, Gemini, OpenAI, Qwen |
| Transcription (STT) | word-level timestamps; duration-billed |
| OCR | documents/images → markdown; per-page pricing |
| Rerank | Cohere-style relevance reorder |
| Moderations | OpenAI-compatible; only provider+IP IAM rules apply |
| Realtime | speech-to-speech over WebSockets |
| Web search | native web-search tool across providers, mapped from provider-native tool schemas |
| Vision / Documents / Reasoning | image/PDF inputs; reasoning-effort controls |

Reasoning is first-class in model metadata: `reasoningEfforts` (`none`/`minimal`/`low`/`medium`/`high`/`xhigh`/`max`), `reasoningMode` (`enabled`/`adaptive`), thinking-tag splitting, etc.

---

## 4. Routing engine (the standout feature)

Call chain: `apps/gateway/src/chat/tools/*` → `packages/actions/src/get-cheapest-from-available-providers.ts` (selection) → `packages/actions/src/compute-provider-scores.ts` (pure scoring) → config via `packages/shared/src/routing-config.ts`.

### 4.1 Model addressing
- Bare id (`gpt-4o`) ⇒ smart routing picks the provider. `provider/model` ⇒ pins provider (no cross-provider fallback). `provider/model:region` ⇒ pins region. `auto` ⇒ optimized auto-selection (cost-first, scales up by context size; `free_models_only`, `no_reasoning` modifiers).

### 4.2 Weighted score (lower wins)
Default weights (`DEFAULT_ROUTING_WEIGHTS`): `price 0.6, imagePrice 1.0, uptime 0.5, throughput 0.05, latency 0.025, cache 0.2`. Per-candidate ratio scores against the best candidate:
- `priceScore = price/minPrice − 1` (free providers score 0 via `minPositivePrice` handling), `uptimeScore = maxUptime/uptime − 1`, `throughputScore`, `latencyScore` (streaming only), `cacheScore = cacheSupported ? 0 : 1` (cache weight only applies when prompt ≥ `cachePromptTokens`, default 5000).
- `score = baseScore + priorityPenalty + uptimePenalty`, where `priorityPenalty = 1 − priority` (default priority 1; some stealth providers 1.2/1.1/0.9) and `uptimePenalty = ((threshold−uptime)/threshold*5)^2` below `uptimePenalty` threshold (default 95) — exponential (90%→~0.07, 70%→~1.73, 50%→~5.61).
- Metrics are a **rolling 60-min window, time-decayed**: last 1 min ×10, last 5 min ×3, rest ×1 (`DEFAULT_ROUTING_HISTORY`, max window 120 min).

### 4.3 Per-request strategy
`routing` body field (bare-model only): `auto` (default), `price`, `throughput`, `latency` — non-auto sets the dominant factor to 0.9, uptime to 0.1, **and forces exploration off**. Combining with a pinned provider → 400. Coding/dev plans allow only `auto`/`price`.

### 4.4 Exploration, stickiness, preference
- **Epsilon-greedy**: 1% of requests pick a random stable provider (`explorationRate` default 0.01, env `EXPLORATION_RATE`); disabled in tests and for sticky/non-auto.
- **Sticky sessions** (`features/sessions.mdx`): session key precedence = `x-session-id` → `x-session-affinity` (opencode) → `prompt_cache_key` → `user` body field (Anthropic: `metadata.user_id.session_id`). First request scored, then pinned; re-pins if pinned uptime < 85%. **Sticky requests never cross-provider fall back.** Also HMAC-hashes the session id into an upstream `prompt_cache_key` for providers that support it (OpenAI, Azure, Meta).
- **Stable per-model preference** ("sticky-ish"): hard-switch if preferred uptime < 85%, soft-switch only if a rival beats it by > 0.15 score margin; expires after 1h. Env: `PREFERRED_PROVIDER_TTL/UPTIME_THRESHOLD/SCORE_MARGIN`.
- `selectionReason` telemetry: `weighted-score | price-only | price-only-no-metrics | random-exploration | session-sticky | stable-preferred`.

### 4.5 Retry / fallback (`chat/tools/retry-with-fallback.ts`)
- `MAX_RETRIES = 2`. Triggers on 5xx / timeout / connection failure; **not** on 4xx or content-filter responses. `X-No-Fallback: true` disables cross-provider reroute (metadata `noFallback:true`); key rotation within the same provider may still occur.
- Three retry modes: same-key (single-provider models, exp backoff `2^(n-1)*1000ms`), alternate-key (same provider, on 401/403/credential errors), cross-provider (needs no pinned provider, `!noFallback`, `!sessionSticky`, retryable, remaining candidates > 0).
- Every attempt logged in `routing[]`: `provider`, `model`, `status_code`, `error_type`, `succeeded`, `credentialSource` (`byok`/`platform`), `apiKeyHash`, `providerKeyId`, `providerKeyLabel`.
- **Low-uptime protection**: pinned provider below `lowUptimeFallbackThreshold` (default 90) auto-reroutes if an alternative exists.

### 4.6 Hybrid / BYOK routing (`chat/tools/hybrid-provider-routing.ts`)
Project `mode`: `api-keys` (BYOK only), `credits` (platform-backed only), `hybrid` (union, prefers BYOK non-rate-limited). Managed DB credentials supersede env-var keys for the same provider.

### 4.7 Dynamic routes (Enterprise) (`features/dynamic-routes.mdx`)
Named, **versioned** routing graphs invoked as `dynamic/<name>`. JSON graph with `entry` + nodes: `conditional` (field source `header`/`body`-dotpath/`metadata` `orgId|projectId|apiKeyId|plan`; ops `eq/neq/in/contains/gt/lt/exists`), `percentage` (weighted, deterministic per session), `model` (terminal, optional ordered provider preference), `end` (400). Draft→publish immutable snapshots with instant rollback; validates reachability, cycles, catalog existence; stale-cache fallback when DB unreachable. Only serves when enabled + published (else 404).

### 4.8 Upstream tuning
`installUpstreamDispatcher()` sets a tuned undici Agent: `UPSTREAM_KEEPALIVE_TIMEOUT_MS 60000`, `UPSTREAM_CONNECT_TIMEOUT_MS 10000`, DNS cache TTL 300000 / 512 items.

---

## 5. Reliability: health, key rotation, stealth error redaction

- **Per-key health** (`lib/api-key-health.ts`, in-memory Map): 3 consecutive errors → 30s blacklist; 5-min sliding window; 401/403 are `PERMANENT_ERROR_CODES` → provider-wide blacklist. Only `{401,403,404,429}` + non-4xx degrade uptime (400 doesn't).
- **Failed-key tracker** (`lib/failed-key-tracker.ts`): per-request set of failed env/BYOK key ids keyed by `(providerId, region)`, shared across chat/embedding retries.
- **Stealth providers** (`lib/stealth-provider-errors.ts`): a provider is "stealth" iff `env.required.baseUrl` is set (undisclosed white-label upstream). Confirmed set of 7: `glacier, iceberg, granite, quartz, permafrost, tundra, avalanche`. For these, raw upstream error text is **redacted** client-side to `"Upstream provider error (NNN Reason)"`; the raw body is kept only in the internal-only `internalErrorDetails` DB column.

---

## 6. Rate / spend / usage limits (four+ independent scopes)

All Redis sliding-window (sorted sets), and **almost all fail open** on Redis/DB error.

1. **Free-model limits** (`lib/rate-limit.ts`): `LOW` tier 5/600s (0-credit) → 20/60s (any credits); `HIGH` tier 50/600s → 100/60s.
2. **Org+path limits** (`lib/org-rate-limit.ts`): window default 60s (`GATEWAY_RATE_LIMIT_WINDOW_SECONDS`), spend-tier multiplier resolved lazily; lifetime spend = `SUM(projectHourlyStats.creditsCost) − refunds`, cached 900s. Includes an **org inflight-concurrency slot limiter** (Redis sorted set with stale-slot reaping).
3. **Provider/model RPM+RPD caps** (`lib/provider-rate-limit.ts`): `rpm`(60s)+`rpd`(86400s), scoped by org×provider×model with `__global__`/`__all_providers__`/`__all_models__` sentinels; `peek` (during scoring) vs `check` (consume).
4. **Spend limits** (`lib/spend-limit.ts`): daily/monthly USD caps for non-enterprise orgs → HTTP 429 (free models exempt); recorded at the single `insertLog` chokepoint.
5. **API-key / member usage limits** (`lib/api-key-usage-limits.ts`): key TTL, all-time `usageLimit`, rolling `periodUsageLimit` (units hour/day/week/month, min 1h max 12mo) → 401; per-member budget → 403 (org-wide developer defaults as fallback).
6. **Backpressure** (`lib/backpressure.ts`): per-pod inflight cap `GATEWAY_MAX_INFLIGHT_REQUESTS` (default 1000) sheds with **HTTP 529** + `Retry-After: 1`.

---

## 7. Cost tracking (`lib/costs.ts`, Decimal.js, ~950 LOC)

Exhaustive per-request cost model surfaced in the response as `usage.cost_details`: `upstream_inference_cost` (+ prompt/completions split), `total_cost`, `input_cost`, `output_cost`, `cached_input_cost`, `cache_write_input_cost` (5m 1.25× / 1h 2× premiums), `request_cost` (flat fee), `web_search_cost`, `image_input_cost`/`image_output_cost`, `data_storage_cost`. Token details include per-TTL cache-creation buckets, reasoning/image/audio/video tokens.

Notable mechanics:
- Exact region match required (no silent base-rate fallback); tiered pricing by prompt-token count; DeepSeek time-of-day peak/off-peak; per-image/per-second/OCR-per-page/audio-per-hour pricing; xAI content-filter fee ($0.05/rejection).
- Service-tier multiplier uses the **served** tier from the response (Google silently downgrades).
- Refusals on Anthropic-family unbilled; only `client_error` + `content_filter` billable on failure.
- `BILL_CANCELLED_REQUESTS` defaults **true** (code references billing-bypass vuln GHSA-724j-f2pf-phf7). Estimated output tokens clamped to `maxOutput` (references a real 1.16M-token over-bill incident); estimation forced off for truncated streams.

---

## 8. Caching

- **Gateway caching** (byte-identical request → $0 replay): cache key from model + messages + all sampling params + tools + response format + system prompt. TTL 10s–1yr, **default 60s**. Markers `x-llmgateway-cache: HIT` header + `metadata.cached: true`; cost fields zeroed (token counts kept). Bypass with `x-no-cache: true`. Works for streaming (reconstructed).
- **Provider cache control**: automatic marker injection on long prompts for OpenAI/Anthropic/Google/DeepSeek/xAI/Alibaba; modes Automatic / Client-managed / Disabled (`providerCacheControlMode`). Anthropic min-cacheable thresholds surfaced as `min_cacheable_tokens` on `GET /v1/models` (4096 for Opus 4.5+/Sonnet 5/Haiku 4.5, down to 1024 for older). Capped at 4 breakpoints; explicit + auto mixing rules per Anthropic ordering. Cached input typically billed 10–25% of input.

---

## 9. Governance & access control

- **API keys** (`llmgtwy_...`, stored as HMAC-SHA256 fingerprints, shown once): all-time + rolling usage limits, TTL, roll/rotate (old → 401), enable/disable.
- **IAM rules** on keys and members (`features/api-keys.mdx`): types `allow/deny_models`, `allow/deny_providers` (incl. `custom` / `custom:<name>`), `allow/deny_pricing` (free-vs-paid, max input/output price), `allow/deny_ip_cidrs` (**Enterprise**, CIDR IPv4/IPv6, client IP from first `X-Forwarded-For`). Same-type allow rules OR; different-type allow rules AND; deny always wins. **Member-level rules are an org-wide ceiling** — key rules can only narrow.
- **Master keys** (Enterprise, `llmgmk_...`, max 10/org): org-scoped bearer tokens over `/v1/master/*` to provision projects, gateway keys, IAM rules, custom providers/models, and pull per-member usage/cost reporting (`GET /v1/master/usage`, groupable by user/model/provider/project/apiKey, CSV/JSON). Usage attributed to key **creator** (no per-caller identity on inference).
- **Compliance-based routing** (Enterprise, `features/compliance.mdx`): restrict routing to providers meeting SOC2/SOC2-Type2/ISO27001/GDPR/no-training/no-logging/no-stealth requirements + HQ-country filter + allow/deny provider&model lists. **Fail-closed** (unknown attribute = non-compliant). Takes precedence over IAM. 403 with a specific message; each block logged as a security event. Provider data-policy metadata lives on `ProviderDefinition.dataPolicy` (`apiTraining/promptLogging/retentionPeriod/soc2/iso27001/gdpr`).
- **Data retention** (`features/data-retention.mdx`): org-level `retentionLevel` `none` (metadata-only, default, free) vs `retain` (full payloads, $0.01/1M tokens). Retention-sensitive fields are **stripped gateway-side before persistence** for non-retaining orgs (`stripRetentionSensitiveLogFields`, core capability). Period **30 days** (Enterprise customizable; Responses API stored 30d regardless). This is the core-vs-EE line: **30-day cap is core; unlimited is EE.**
- **Guardrails** (Enterprise, `ee/guardrails`): org-wide (project can fully override). All **regex/heuristic, no external ML/LLM vendor** (no Presidio/Lakera). System rules: prompt-injection, jailbreak, PII (SSN/card w/ Luhn/email/phone/IP/passport/license), secrets (AWS/GitHub/Slack/Stripe/OpenAI/JWT/bearer + Shannon-entropy + placeholder filtering), file-type MIME restriction, document-leakage. Custom rules: blocked-terms (exact/contains/regex), custom-regex (ReDoS-guarded), topic-restriction. Actions: `block`/`redact`/`warn`/`allow`. Violations logged with matched pattern (truncated 100 chars). Security-events dashboard.
- **Audit logs** (`ee/audit`): ~70 tracked actions (org/project/team/key/provider-key/master-key/custom-model/billing/SSO/SCIM), diff-style `changes` metadata. **Writes are unconditional (core) — reads are Enterprise-gated.** Retention 90 days. No hash-chaining/tamper-evidence.
- **SSO/SCIM** are in **core** `packages/db` (not `ee/`): `ssoProvider` (okta/entra/generic, enforced, domain-verified), SCIM tokens/groups/directory sync, role mappings, default projects.
- **Multi-org admin console** (`ee/admin`, white-label license): a **vendor-operator** control plane (billing overrides, credit gifting, fraud/`flagged-accounts` review, provider-credential management, routing/mapping stability monitoring, bulk-block) — not customer-facing sub-org management.

---

## 10. Provider & model catalog (`packages/models`)

- **53 provider ids** (`providers.ts`): `llmgateway, openai, anthropic, google-ai-studio, glacier, iceberg, granite, google-vertex, vertex-openai, vertex-anthropic, quartz, avalanche, groq, cerebras, xai, deepseek, alibaba, novita, atlascloud, aws-bedrock, aws-mantle, azure, azure-ai-foundry, azure-anthropic, zai, moonshot, baidu, permafrost, perplexity, nebius, mistral, canopywave, inference.net, together-ai, scx-ai, scx-ai-gp, custom, nanogpt, bytedance, minimax, embercloud, meta, sakana, tundra, xiaomi, deepinfra, reve, elevenlabs, runware, gonka24, fireworks, ranoai, consensusprotocol`.
- **`ProviderDefinition`** carries: env requirements (`apiKey`/`baseUrl`/exclusive groups), `streaming`/`cancellation`, `priority` (routing multiplier), `contentFilter` flag, `regionConfig` (region endpoint maps, Bedrock cross-region prefixes global./us./eu./apac., shared-credential-across-regions), `serviceTiers` (flex/priority multipliers), `headquarters` (ISO country), `dataPolicy` (the compliance metadata), `forwardsSafetyIdentifier`.
- **`ModelDefinition`** (per model): `family`, `providers: ProviderModelMapping[]`, `free`, `output` array (`text/image/video/embedding/audio/ocr/transcription/rerank`), `stability` (`stable/beta/unstable/experimental`), reasoning/system-role flags.
- **`ProviderModelMapping`** (the pricing/capability record, ~600 LOC of optional fields): `contextSize`/`maxOutput`/`quantization`/`region(s)`; a large family of string-typed prices (input/output/cached/cache-read/cache-write-5m/1h/image/audio/per-image/per-second/OCR-page/request/web-search/content-filter, plus `pricingTiers` and DeepSeek `peakPricing`); capability flags (`streaming|"only"`, `vision`, `audio`, `document`, `reasoning` + `reasoningEfforts`/`reasoningMode`, `tools`/`parallelToolCalls`/`supportedToolChoices`, `jsonOutput`/`jsonOutputSchema`, `webSearch`, `supportsResponsesApi`, `serviceTiers`); modality surfaces (`imageGenerations`, `embeddings`, `speechGenerations`, `realtime`, `transcriptions`, `ocr`, `rerank`, `videoGenerations` + supported sizes/durations/voices); lifecycle (`deprecatedAt`, `deactivatedAt`, `test`). Catalog is 26+ per-family files concatenated into a typed const array.

---

## 11. Business model note: embeddable payments (resale)

`features/embeddable-payments.mdx` + `wallet`/`walletLedger`/`endCustomer`/`endUserSession` tables: a Payments SDK lets a customer embed **per-end-user wallets** into their own site; end-users buy credits and pay per request, billed through LLM Gateway, with the customer's **markup as margin** (`endUserMarkupPercent`, `endUserTopUpBonusPercent`, `platform_secret`/`end_user_customer` key types, `walletLedger` split into `grossPaid`/`platformFee`/`developerMargin`/`netCredited`). This is a resale/white-label monetization primitive with no analog in Vercel/agentgateway/NetBird/helsinki.

---

## 12. EE vs core split (precise)

| Capability | Tier |
|---|---|
| Gateway, all routing (weighted score, sticky, retry/fallback, hybrid/BYOK), all modalities, cost tracking, gateway+provider caching | **Core (AGPL)** |
| Rate/spend/usage limits, backpressure, IAM rules (models/providers/pricing), per-member budgets, period limits | **Core** |
| Data-retention stripping + 30-day retention, SSO/SCIM, audit-log **writes** | **Core** |
| Per-project routing config overrides, dynamic routes, IP-CIDR IAM rules | **Enterprise** |
| Compliance-based routing, guardrails (config+enforcement), audit-log **reads**, master keys, unlimited/custom retention, custom-model-catalog management | **Enterprise** |
| White-label multi-org admin console | **White-label license** |

Licensing enforced by offline JWT verification; `hasOrganizationEnterpriseAccess(orgId, plan)` requires `plan === "enterprise"` + active/grace license matching the org (or white-label/dev).

---

## 13. Positioning vs the other three (and helsinki)

| Axis | Vercel AI Gateway | agentgateway | NetBird Agent Network | **LLM Gateway** |
|---|---|---|---|---|
| Shape | Hosted SaaS | Self-host Rust data plane | Feature on WireGuard mesh | **Self-host TS app (AGPL) + hosted** |
| LLM egress routing | Deep | Deep (virtual models, CEL) | Focused (allowlist) | **Deepest config-surface of the OSS options** (weighted score + exploration + sticky + dynamic graphs) |
| Modalities | Broad | LLM shapes + rerank/realtime | Chat only | **Broadest** (chat/embed/image/video/speech/STT/OCR/rerank/moderation/realtime/websearch) |
| MCP / A2A / ACP | MCP via AI SDK | Deepest MCP + A2A proxy | None | **MCP client-facing tools only; no MCP-proxy, no A2A/ACP** |
| Guardrails | None | Broadest (moderation, PII, webhook, streaming) | Regex PII + model allowlist | **Regex/heuristic system+custom rules (EE), no external vendor** |
| Governance | Spend caps, ZDR, allowlist | Virtual keys, CEL RBAC | Budget rules, network-layer identity | **IAM rules + compliance routing + master keys + audit (EE)** |
| Identity model | API key / OIDC | JWT/OAuth virtual keys | WireGuard peer + IdP | **API key / master key; per-caller identity NOT on inference** |
| Cost/telemetry | Dashboard + REST | OTel GenAI semconv + Prometheus | Access logs | **Rich per-request `cost_details` + hourly rollups; no OTel GenAI export noted** |
| Business model | Usage + ZDR add-ons | OSS/foundation | OSS/self-host | **OSS + hosted + embeddable-payments resale** |

**Where LLM Gateway leads the field:** breadth of modalities, the richness/tunability of the pure-LLM routing engine (weighted multi-factor + exploration + versioned dynamic-route graphs), compliance-gated routing with per-provider data-policy metadata, and the resale/wallet business primitive. **Where it is thin vs helsinki/agentgateway:** it has no agent-coordination protocol surface — no MCP transport/federation as a data plane, no A2A task lifecycle, no ACP, no discovery registry, no durable execution. It is a superb *model egress* gateway, not an *agent* gateway.

---

## 14. What helsinki (TrogonAI) could take from it

Reinforces and sharpens the P0/P1 items already in `VERCEL_AGENTGATEWAY_RESEARCH.md` Part 5, with concrete implementable patterns:

1. **Weighted multi-factor routing math** (`compute-provider-scores.ts`) is a clean, self-contained reference for the P1 "model routing policies" item — ratio-scores + exponential uptime penalty + priority penalty, with a time-decayed rolling metrics window. Directly portable in spirit.
2. **Response `cost_details` schema + hourly rollups** is a good target shape for the P0 "token/cost accounting" item; pair with the GenAI OTel semconv work (helsinki already leads on semconv codegen, so it can export what LLM Gateway only dashboards).
3. **Compliance-gated routing with per-provider `dataPolicy` metadata** is a differentiator neither Vercel nor NetBird matches and fits helsinki's SpiceDB/policy strengths — model providers as policy-bearing resources.
4. **Multi-scope limit design** (org / api-key / provider-model / member, all fail-open Redis windows + inflight concurrency slots) maps onto extending helsinki's existing per-caller inflight gate into token/spend budgets.
5. **Data-retention stripping before persistence** (`stripRetentionSensitiveLogFields`) is a clean privacy default mirroring NetBird's capture-pointer pattern; worth adopting for helsinki's log path.
6. **Versioned dynamic-route graphs** (draft→publish→immutable version→instant rollback, with stale-cache fallback) is a strong control-plane pattern for any policy/routing config helsinki ships.

**Explicitly not a threat to helsinki's lead:** LLM Gateway has no A2A task durability/push, no ACP, no MCP-as-data-plane federation, no discovery catalog, no event-sourced deciders/scheduler, no NATS-account multi-tenancy. helsinki's agent-coordination depth is orthogonal to what LLM Gateway does well (model egress).

---

## Appendix: methodology & sources

Produced 2026-08-27 by a multi-agent research run against `theopenco/llmgateway` via the GitHub API: three parallel agents mapped (a) the feature/developer docs under `apps/docs/content`, (b) the gateway routing/reliability/cost engine under `apps/gateway/src/lib` + `chat/tools` + `packages/{actions,shared,models}`, and (c) the enterprise edition under `ee/` plus the Drizzle schema in `packages/db`. Claims cite file paths and exact identifiers for re-verification. Feature state is as of the research date; the project ships frequently, so re-check specifics before acting.

Primary source files (representative): `README.md`; `apps/docs/content/{overview.mdx,self-host/*,features/*,developers/*}`; `apps/gateway/src/lib/{routing-config-loader,preferred-provider,rate-limit,org-rate-limit,provider-rate-limit,spend-limit,api-key-usage-limits,backpressure,costs,api-key-health,failed-key-tracker,stealth-provider-errors,compliance,iam}.ts`; `apps/gateway/src/chat/tools/{retry-with-fallback,hybrid-provider-routing}.ts`; `packages/shared/src/{routing-config,enterprise-license}.ts`; `packages/actions/src/{compute-provider-scores,get-cheapest-from-available-providers}.ts`; `packages/models/src/{providers,models,types,provider}.ts`; `packages/db/src/{schema,log-retention,member-budget,provider-key-allowed-models,api-key-period-limit}.ts`; `ee/{README.md,LICENSE,guardrails/src/**,audit/src/**,admin/src/**}`.
