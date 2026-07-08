# AI Gateway Landscape: Vercel AI Gateway + agentgateway vs TrogonAI (helsinki)

> Research date: 2026-07-04. All product claims were fetched live from official docs/changelogs and adversarially verified by independent fact-checking agents. Source URLs inline.
>
> **Naming note:** "Vercel agentgateway" conflates two distinct products:
> 1. **Vercel AI Gateway** (`vercel.com/docs/ai-gateway`): Vercel's hosted LLM gateway (unified model API, routing, spend).
> 2. **agentgateway** (`agentgateway.dev`): an open-source Rust data plane for agent traffic (MCP, A2A, LLM APIs). Created by Solo.io (March 2025), donated to the **Linux Foundation** (Aug 2025), now hosted by the **Agentic AI Foundation (AAIF)**. It is NOT a Vercel product and, contrary to common belief, not a CNCF sandbox project (the CNCF-affiliated project is **kgateway**, which integrates agentgateway as of kgateway v2.1).
>
> Both are documented below since both are directly relevant to helsinki.

---

## Part 1: Vercel AI Gateway, full feature catalog

### 1.1 Core API and model access

| Feature | Detail |
|---|---|
| Unified multi-provider API | Single endpoint `https://ai-gateway.vercel.sh/v1`, one API key, ~300 models across ~31-45 provider slugs. ([docs](https://vercel.com/docs/ai-gateway)) |
| Four API surfaces | OpenAI Chat Completions, OpenAI Responses, Anthropic Messages (`/v1/messages`, incl. `count_tokens`), and OpenResponses.org spec. Official OpenAI/Anthropic SDKs work by changing base URL. ([docs](https://vercel.com/docs/ai-gateway/sdks-and-apis)) |
| Model slugs | `creator/model-name` (e.g. `anthropic/claude-opus-4.8`); same model hosted by several providers exposes per-provider endpoints. |
| Dynamic model discovery | `GET /v1/models` (no auth, filterable by `type`: language/embedding/reranking/image/video), `GET /v1/models/{creator}/{model}/endpoints`, `gateway.getAvailableModels()` with pricing incl. tiered/cache pricing. |
| AI SDK integration | Default provider for AI SDK v5/v6 when model given as plain string; `@ai-sdk/gateway` package, `createGateway()` for custom instances. |
| Framework/language ecosystem | Python via OpenAI/Anthropic SDKs, Pydantic AI, LangChain, LlamaIndex, Mastra, LiteLLM, LangFuse integrations. ([docs](https://vercel.com/docs/ai-gateway/ecosystem/framework-integrations)) |
| Auth: API keys | Dashboard-created, never expire unless revoked, per-key spend budgets. |
| Auth: OIDC tokens | Auto-issued `VERCEL_OIDC_TOKEN` on Vercel deployments, 12h validity, no key management. |
| Streaming | SSE on all API surfaces; structured outputs work under streaming. |
| Tool/function calling | Full OpenAI function-calling spec passthrough (`tools`, `tool_choice`, `finish_reason: tool_calls`). |
| Structured outputs | `response_format: json_schema` per OpenAI spec. |
| Prompt caching | Explicit passthrough (`cache_control` for Anthropic/MiniMax) plus automatic implicit caching; `has: ['implicit-caching']` capability filter. ([docs](https://vercel.com/docs/ai-gateway/models-and-providers/automatic-caching)) |
| Reasoning controls | Cross-provider `reasoning: {effort}` mapping (none..xhigh), Anthropic native `thinking` param. |
| Built-in web search tools | `gateway.tools.perplexitySearch()` ($5/1k), `exaSearch()` ($7/1k), `parallelSearch()` ($5/1k) usable with any model, plus native provider search passthrough. ([docs](https://vercel.com/docs/ai-gateway/models-and-providers/web-search)) |
| Service tiers | Provider tier selection (e.g. OpenAI flex/priority). |

### 1.2 Modalities (beyond chat)

- **Image generation** (`experimental_generateImage`): GPT Image, Imagen, Flux, Recraft; generation, editing, variations.
- **Video generation** (`experimental_generateVideo`, AI SDK 6+): Veo 3.1, Kling, Wan, Seedance; capability tags `t2v`/`i2v`/`r2v`/`motion-control`; duration/aspect/resolution/audio params.
- **Embeddings** (`embed`/`embedMany`, OpenAI-compatible `/embeddings` with auto-mapped `dimensions`).
- **Reranking** (`rerank`, e.g. `cohere/rerank-v3.5`, AI SDK only).
- **Speech-to-Text / Text-to-Speech** (beta, OpenAI models only, no streaming).
- **Realtime** (low-latency two-way voice).
([docs](https://vercel.com/docs/ai-gateway/modalities))

### 1.3 Routing and reliability

| Feature | Detail |
|---|---|
| Default routing | Dynamic provider choice by recent uptime + latency. |
| `order` | Explicit provider attempt sequence. |
| `only` | Restrict to a provider subset (applied before `order`). |
| `sort` | Rank providers by `cost`, `ttft`, or `tps` (May 2026); response metadata exposes `executionOrder`, `metrics`, `deprioritizedProviders`. Health-aware: degraded providers penalized, down providers last. |
| Automatic failover | Retries across providers; every attempt logged in `providerMetadata.gateway.routing.modelAttempts[].providerAttempts[]`. |
| Model fallbacks | `models: [...]` ordered backup models (cross-creator), Nov 2025. |
| Per-provider timeouts | `providerTimeouts` (1s-789s) trigger failover if no first token; BYOK-only for now (Mar 2026). |
| Routing rules (beta) | Team-wide rewrite (model substitution) and deny (403) rules via `vercel ai-gateway rules` CLI, no code changes (Jul 2026). |
| BYOK failover | BYOK failure auto-falls back to Vercel system credentials (billed to credits). |
| Uptime/status monitoring | Per-provider and gateway-wide uptime views (1H/1D/1W), color-coded health, `uptime_last_15m/1h/1d` via endpoints API; live TTFT/TPS metrics feed `sort`. |
| Rate limits | Free tier: per-model limits, HTTP 429; paid tier raises limits. Numeric thresholds unpublished. |

### 1.4 Cost, BYOK, spend governance

- **Pay-as-you-go, 0% token markup** (incl. BYOK); credits with auto top-up; $5/month free tier credit. ([pricing](https://vercel.com/docs/ai-gateway/pricing))
- **BYOK**: team-wide credentials, request-scoped `byok` option, multiple credentials tried in order, Azure `modelMappings`, "Test Key" validation.
- **API Key Budgets**: per-key spend caps with daily/weekly/monthly refresh.
- **Custom Reporting** (add-on): tag/user-ID/quota-entity attribution, group-by model/user/tag/provider/credential type ($0.075/1k writes, $5/1k queries).
- **REST spend APIs**: `GET /v1/credits`, `GET /v1/generation?id=` (per-generation cost/latency/token breakdown incl. cached/reasoning tokens and BYOK distinction).
- **App attribution**: track which apps/clients drive traffic.

### 1.5 Observability

- Dashboard: requests by model, TTFT, token counts, spend over time (team/project scope).
- Request summaries grouped by project and API key (count, avg tokens, P75 duration/TTFT, cost); sortable/exportable request logs.
- Response metadata: `routing` (attempts, planningReasoning), `cost`, `marketCost`, `generationId` for audit lookup.
- No OTel export of gateway telemetry documented; observability is dashboard/API-centric.

### 1.6 Security and compliance

- **Zero Data Retention**: Vercel layer never retains prompts; team-wide ZDR ($0.10/1k, Pro/Enterprise) or per-request `zeroDataRetention: true` (free); providers without ZDR agreement treated as non-compliant; BYOK keys excluded unless marked.
- **Disallow prompt training** (subset of ZDR).
- **Provider allowlist** (team-wide, $0.10/1k, Owner-managed; ANDs with per-request `only`; new providers default-disabled).
- Note: no built-in guardrails/content moderation, no PII redaction, no region pinning documented. This is Vercel AI Gateway's biggest governance gap vs enterprise gateways.

### 1.7 Adjacent Vercel "Agent Stack" products (not the gateway itself)

| Product | What it does |
|---|---|
| MCP support in AI SDK (`@ai-sdk/mcp`, stable in AI SDK 6) | Agent hosts consume MCP tools; HTTP transport for remote servers. |
| `mcp-handler` | Turn Next.js/Nuxt/Svelte apps into remote MCP servers (Streamable HTTP + SSE). |
| Vercel MCP | First-party OAuth MCP server exposing Vercel platform data. |
| `mcp-to-ai-sdk` | Generate static typed AI SDK tools from MCP schemas (supply-chain hardening). |
| Vercel Sandbox | Firecracker microVMs for untrusted agent code (GA Jan 2026). |
| Workflow Development Kit | Durable async/await TypeScript (and Python beta), replay/resume for minutes-to-months. |
| Vercel Queues | Durable append-only topics, consumer groups (beta). |
| Vercel Agent + Investigations | Dashboard AI teammate: code review, anomaly root-cause analysis. |
| AI SDK Agent abstraction | `ToolLoopAgent`, `stopWhen`, `prepareStep`, DevTools inspector. |
| eve | Open-source agent framework, "Next.js for agents" (Ship 2026). |
| Vercel Connect | Short-lived task-scoped credentials for agent-to-service access (replaces env-var API keys). |
| Chat SDK | One agent codebase deployed as Slack/Teams/etc. bots. |
| BotID, Fluid compute, Coding-agents recipes | Bot defense; Active-CPU pricing; route Claude Code/Codex through the gateway via env vars for unified spend. |

---

## Part 2: agentgateway (Linux Foundation / AAIF), full feature catalog

Rust data plane for agent traffic; Apache 2.0; ~3.7k stars; contributors incl. Microsoft, Apple, AWS, Adobe; benchmark claims ~500k QPS, sub-0.2ms P99 at 30k QPS. Releases: v1.0 (Mar 2026), v1.2 (May 2026), v1.3 (Jun 2026).

### 2.1 LLM gateway

- **9 normalized API route types with per-provider translation**: Chat Completions, Responses, Messages (Anthropic), Embeddings, Rerank (Cohere-compatible `/v2/rerank`), Anthropic Token Count, OpenAI Realtime (WebSocket, with token-usage extraction), Models, and raw Passthrough (opaque/detect modes). Any client-facing shape translated to what the provider natively speaks (Bedrock via Converse API). ([docs](https://agentgateway.dev/docs/standalone/latest/llm/about))
- **19 first-class providers + custom** (13 added in v1.3: Mistral, HF, Cohere, Groq, Fireworks, DeepSeek, xAI, Together, OpenRouter, Cerebras, DeepInfra, Baseten, Ollama).
- **Virtual models** (v1.3): synthetic model names with routing strategies: **weighted** (A/B traffic split), **failover** (ordered fallback on errors/429), **conditional** (CEL on user tier/prompt length/headers); P2C load balancing across backends; CEL `unhealthyCondition` for eviction.
- **Virtual API keys**: per-key metadata (user_id, tier) in K8s Secrets, token budgets via rate-limit policies, per-key Prometheus cost labels.
- **Model-cost catalog**: JSON per-1M-token rates (input/output/cacheRead/cacheWrite/reasoning/audio, context-length tiers), `agctl costs import` from models.dev; `llm.cost` exposed in CEL/logs/traces/metrics.
- **Token budgets + rate limiting**: token-bucket budgets per key/user (`unit: Tokens`, HTTP 429, local or global mode), `x-ratelimit-*` headers; separate HTTP and MCP tool-call rate limiting.

### 2.2 MCP connectivity (deepest in the market)

- **Static / Dynamic / Virtual MCP backends**: fixed address, label-selector-based (rolling swap), or **federation**: many MCP servers behind one endpoint, `tools/list` fanned out and aggregated with per-server tool namespacing.
- **Stateful MCP sessions**: implements MCP `Mcp-Session-Id` session model with stateful/stateless modes and session affinity.
- **Resource subscribe/unsubscribe + resource multiplexing** across federated servers.
- **MCP authorization**: OAuth 2.0 resource server per MCP auth spec (`/.well-known/oauth-protected-resource/mcp`), eager connect-time auth; first-class Keycloak, Auth0, Okta integrations.
- **MCP guardrails (ExtMCP)**: external gRPC policy server hooks (CheckRequest/response) to gate and mutate `tools/call`, `tools/list`.
- **Tool-level RBAC** via CEL rule sets.

### 2.3 A2A connectivity

- Proxies A2A agents (standalone YAML config or K8s `AgentgatewayBackend` CRD with `a2a` type; legacy `appProtocol: kgateway.dev/a2a`).
- Agent card served at `/.well-known/agent.json` with URL rewriting to point through the gateway.
- JSON-RPC streaming (`message/stream`), playground skill discovery.
- Note: A2A support is a thin proxy layer (routing, card rewrite, policy attach); no task store, no push notifications, no A2A-native event replay.

### 2.4 Policy engine and guardrails

- **Embedded CEL runtime** across all policy phases: conditional policies (ordered variants, first-match), RBAC allow/deny rule sets, retry `condition`/`precondition`, failover `unhealthyCondition`, direct-response `bodyExpression`, rate-limit descriptors, transformations, log/trace field extraction. In-UI **CEL playground** (runs the production CEL runtime) + static CEL context explorer.
- **Guardrails**: layered request+response chains with pass/reject/mask outcomes; regex + built-in PII detectors (credit cards, SSN, email, phone); OpenAI Moderation API; AWS Bedrock Guardrails (provider-agnostic); Google Model Armor; custom webhook guardrail API with `failureMode: FailOpen|FailClosed`; streaming (SSE/Realtime WebSocket) guardrails as of v1.3; shared/reusable guardrail declarations.
- **Security**: JWT/OAuth authn, external-authorization caching, per-model authorization for `/v1/models` listing.

### 2.5 Observability and operations

- **OTel tracing** (static/global and per-listener dynamic; CEL-computed span attributes), **GenAI semconv** (`gen_ai.usage.input_tokens/output_tokens`, `gen_ai.request.model`...), `agentgateway_gen_ai_client_token_usage` Prometheus histogram.
- Prometheus data-plane metrics (`agentgateway_requests_total`, MCP `tool_calls_total`, `tool_call_errors`, resource/prompt counters) + control-plane metrics (reconciliation, xDS auth/NACKs).
- Turnkey reference stack: Loki + Tempo + Prometheus + 3 OTel collectors + Grafana dashboards.
- **Admin UI** (port 15000): standalone read-write (configure models, providers, MCP servers, policies, guardrails, costs, virtual keys), K8s read-only; **LLM playground** (chat tester with latency/token metadata) and MCP/A2A playground.
- **agctl CLI**: `proxy trace` (request tap), `proxy config`, log-level control, `costs import`.

### 2.6 Deployment

- Standalone binary / Docker; Kubernetes with **own control plane** implementing **Gateway API v1.5** (standard+extended+experimental conformance), `AgentgatewayPolicy` CRD, Helm OCI charts, documented ArgoCD/FluxCD flows.
- kgateway v2.1 integration (CNCF); Istio 1.30 experimental `istio-agentgateway` GatewayClass; ambient waypoint direction.
- **Gateway API Inference Extension**: routes to `InferencePool`/`InferenceModel`; first GIE v1.4.0-conformant gateway; llm-d Endpoint Picker for KV-cache/GPU-utilization/LoRA/queue-depth-aware routing, prefill/decode disaggregation.

---

## Part 3: What helsinki (TrogonAI) has today

Verified against code by five exploration agents; representative crate/file evidence is cited inline.

### 3.1 Strengths (some exceed both gateways)

| Capability | Status | Notes |
|---|---|---|
| A2A protocol depth | Implemented | Full task lifecycle, message/stream, `tasks/resubscribe` with JetStream event replay, push notifications over HTTPS/NATS/JetStream with retry, idempotency keys, DLQ, dedup. **Richer than agentgateway's A2A proxy** (which has no task store or push delivery). |
| A2A HTTP binding | Implemented | JSON-RPC + REST + SSE (`a2a-nats-http`), agent-card well-known endpoint, A2A version/extension header negotiation. |
| Ingress policy stack | Implemented | Tier 1 declarative bundles + SpiceDB relationship authz; Tier 2 per-skill CEL; Tier 3 wasm-driven redaction/PII rewrite. SpiceDB-backed relational authorization is beyond agentgateway's CEL RBAC. |
| AuthN / multi-tenancy | Implemented | NATS auth-callout minting scoped user JWTs from OIDC / mTLS / (deprecated) API keys; tenancy = NATS account, `aud`-validated; AAuth (draft-hardt) PoP verification partial (Off/Shadow/Enforce). |
| MCP transport | Implemented | NATS transport for official `rmcp` SDK covering tools, resources (incl. subscribe), prompts, sampling, elicitation, roots, completion, logging, tasks, cancellation; Streamable HTTP endpoint (`mcp-nats-server`); stdio bridge. |
| ACP protocol | Implemented | Near-full agent+client surface over NATS, Streamable HTTP + WebSocket gateway, stdio bridge. **Neither Vercel nor agentgateway supports ACP.** |
| Shared JSON-RPC/NATS codec | Implemented | ADR 0011 content-mode codec shared by all three protocols. |
| Discovery | Implemented | ARD-spec catalog/registry (`/.well-known/ai-catalog.json`, search/explore HTTP API) + A2A catalog on NATS KV with SpiceDB-gated views and audit trail. Neither comparison product has a registry. |
| Webhook ingress | Implemented | 11 SaaS sources + Discord WS with per-source signature verification onto JetStream. Out of scope for both comparison products. |
| Durable execution | Implemented | Event-sourced deciders (native + zero-import wasm components with fuel/memory limits), simulation + YAML conformance CLI, scheduler with RRULE/cron reconciliation. Comparable in spirit to Vercel Workflows/Queues, self-hosted. |
| Telemetry discipline | Implemented | Weaver-generated semconv registry, dylint-enforced constants, OTLP traces/metrics/logs. Stronger governance than either product's telemetry story. |
| Backpressure | Implemented | JetStream pull-consumer flow control with per-caller inflight gate. |

### 3.2 Known internal gaps (from code, independent of the comparison)

- Scheduler and ARD registry are libraries/demos without deployed service binaries; wasm decider host runtime is built and tested but no service wires it to live NATS traffic yet.
- Health endpoints only on `trogon-gateway` (hardcoded 200s); Docker Compose only, no Kubernetes; no collector/Grafana provisioning; only 7 metrics registered.
- No TLS termination in-process (delegated by design, ADR 0015 covers libraries only).
- a2a-bridge (HTTPS cross-network) partial, default stub transport.
- No cross-protocol bridging (A2A <-> ACP <-> MCP).

---

## Part 4: Gap analysis, what we do NOT have

### Category A: LLM egress gateway (entirely absent)

Nothing in the codebase touches LLM providers, models, tokens, or inference cost (confirmed by grep sweeps and ADR review). Everything in Part 1 and section 2.1 is a gap:

1. Unified LLM API surface (OpenAI/Anthropic-compatible endpoints).
2. Provider adapters + API-shape translation.
3. Model routing: failover chains, weighted split, cost/latency-based sort, conditional routing.
4. BYOK / provider credential management (we manage caller credentials, not provider credentials).
5. Token usage accounting (GenAI semconv metrics), model-cost catalog, per-request cost computation.
6. Budgets and spend limits (per key / per tenant / per agent).
7. Token-based rate limiting.
8. Prompt caching passthrough, provider uptime tracking, ZDR/allowlist-style provider governance.
9. Modalities (embeddings/image/video/rerank/speech) and model catalog/discovery for LLMs.

### Category B: MCP governance (transport exists, gateway features do not)

10. MCP federation / virtual MCP (aggregate many servers, tool namespacing).
11. Tool-level authorization on MCP traffic (our Tier 1/2 stack guards A2A ingress only).
12. MCP session affinity at the HTTP edge; MCP guardrail hooks; MCP tool-call rate limiting; MCP OAuth resource-server metadata.

### Category C: Guardrails breadth

13. We have wasm redaction (arguably a better extension point) but no moderation-API integration, no Bedrock Guardrails / Model Armor connectors, no webhook guardrail contract, no fail-open/closed semantics, no streaming guardrails, no reusable/shared guardrail declarations.

### Category D: Operations and DX

14. No admin UI or playground (LLM/MCP/A2A/CEL testers).
15. No ops CLI (config dump, request tap/trace, log-level control).
16. No Kubernetes story (Helm, Gateway API conformance, GitOps flows).
17. Sparse metrics; no request-log/usage dashboards; no reference observability stack; health checks incomplete.
18. No HTTP rate limiting or retry/timeout/failover policy knobs at the gateway edge.

### Category E: Ecosystem conveniences (lower relevance)

19. Coding-agent recipes (route Claude Code/Codex through a gateway), web-search tools, uptime status pages, GPU/KV-cache-aware inference routing (relevant only if we serve self-hosted models).

---

## Part 5: What we should introduce (prioritized)

### P0: adopt (high leverage, natural fit with NATS + existing crates)

1. **LLM egress gateway service** (`trogon-llm-gateway` or extend a2a-gateway scope). Governed path for agent-to-model calls: OpenAI Chat Completions + Anthropic Messages compatible HTTP surface, provider adapters, per-tenant provider credential store keyed on the existing NATS account/CallerId identity. This is the single biggest gap: the platform coordinates agents but has no visibility or control over their model calls.
2. **GenAI OTel semantic conventions**. Add `gen_ai.*` spans/metrics (token usage histogram, request/response model attrs) to the Weaver registry. Cheap, extends an existing strength, and prerequisites cost tracking.
3. **Token/cost accounting + budgets**. Model-cost catalog (agentgateway's models.dev import is a good pattern), per-caller/per-tenant token budgets enforced at the gateway (extend the existing per-caller inflight gate into a token-bucket policy), spend metrics labeled by CallerId/account.
4. **Virtual MCP / federation + tool-level authz**. Aggregate multiple `mcp-nats` servers behind one endpoint with tool namespacing, and extend Tier 1/Tier 2 policy evaluation to `tools/call`. We already have every building block (rmcp transport, CEL, SpiceDB).

### P1: adopt next

5. **Model routing policies**: failover chains and CEL-conditional routing (reuse `cel-interpreter` from Tier 2); weighted split later. Vercel's `order`/`only`/`sort` is a good config vocabulary.
6. **Generalized guardrail chain**: promote a2a-redaction's wasm engine into an ordered request/response guardrail pipeline (pass/reject/mask, fail-open/closed, streaming support), add a webhook guardrail contract for external moderation services. Wasm-native guardrails would be a differentiator vs both products.
7. **Rate limiting**: request-based HTTP limits plus token-based LLM limits, local and NATS-coordinated global modes.
8. **Operational baseline**: real readiness checks (NATS/JetStream dependency probes) on all services, request-log/usage projections, reference OTel collector + Grafana provisioning in devops/, per-provider uptime metrics once the LLM gateway exists.

### P2: adopt later / watch

9. **Admin UI + playgrounds** (A2A card explorer, MCP tool tester, CEL/policy playground; our SpiceDB/CEL stack makes a policy playground especially valuable).
10. **Ops CLI** (`trogonctl`: config dump, request tap on NATS subjects, log-level control, cost import).
11. **Kubernetes packaging** (Helm charts; evaluate Gateway API alignment; ArgoCD/FluxCD docs).
12. **BYOK-with-fallback, ZDR/provider-allowlist analogues, prompt-caching passthrough** once the LLM gateway matures.
13. **Gateway API Inference Extension / llm-d awareness** only if self-hosted model serving enters scope.

### Explicitly not gaps (we lead here)

A2A task durability + push notifications with DLQ, ACP support, SpiceDB relational authorization, wasm redaction extension point, ARD/A2A discovery catalogs, webhook ingress, event-sourced deciders + scheduler, semconv codegen with lint enforcement, NATS-account multi-tenancy.

---

## Appendix: methodology

Produced on 2026-07-04 by a multi-agent research run: 5 code-exploration agents mapped this repository (evidence cited as file paths above), 10+ web-research agents fetched official Vercel and agentgateway docs/changelogs, each topic was then adversarially fact-checked by an independent verifier against primary sources, and a completeness critic triggered follow-up passes for uncovered areas. Feature claims reflect product state as of the research date; both products ship changes frequently, so re-verify specifics against the linked sources before acting on them.
