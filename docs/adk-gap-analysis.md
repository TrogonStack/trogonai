# trogonai vs ADK + Vertex AI — Gap Notes

> Source: https://docs.cloud.google.com/gemini-enterprise-agent-platform/build/adk · https://adk.dev

---

## What ADK is

Open-source framework (Python / TypeScript / Go / Java) for building agents. `pip install google-adk`. The **Vertex AI Agent Platform** is the managed cloud layer on top: Memory Bank, RAG Engine, Agent Gateway, Model Armor, Governance, Production Eval Service.

---

## How far we are — by area

### Agent definition
ADK defines an agent as a single declarative object with identity, bound instruction, output schema, model config, and all callbacks attached. trogonai's `AgentLoop` is an anonymous runtime struct — the system prompt and tools are passed per call, not bound to the agent. No `name`, no `description`, no `output_schema`, no history control, no `generate_content_config`. ADK also supports `global_instruction` — a static preamble injected into every sub-agent in a hierarchy without repeating it per agent — and a `planner` slot (built-in `PlanReAct` or custom `BasePlanner`) that makes the agent generate an explicit plan before acting. trogonai has neither.

### Workflow composition
ADK has `SequentialAgent`, `ParallelAgent`, and `LoopAgent` as first-class composable types — deterministic, no LLM in the control flow, freely nestable. trogonai has none of these. Pipelines are wired at the NATS infrastructure level, not in code.

### Custom orchestration
ADK's `BaseAgent` lets you implement `_run_async_impl(ctx)` — full access to session state, sub-agent calls, and custom branching in code. trogonai has no equivalent. Custom logic lives in ad-hoc NATS handler code.

### Runner — unified execution entry point
ADK's `Runner(agent, app_name, session_service, memory_service, artifact_service)` is the single entry point for all agent execution. All backends are swappable: `InMemorySessionService` for dev, `VertexAiSessionService` or `DatabaseSessionService(db_url)` (SQLite/PostgreSQL/MySQL) for prod — same agent code, no changes. It yields typed `Event` objects with helpers like `event.is_final_response()` and `event.get_function_calls()`. trogonai has no Runner — execution is driven by NATS message consumption, and there are no swappable service backends.

### Multi-model support
ADK accepts any model as a string: `model="gemini-2.0-flash"`, `model="ollama/llama3.2"`, `model="openai/gpt-4o"` — provider switches with no code change. trogonai supports Claude (native), xAI Grok (separate crate). Switching provider requires changing crates.

### Multi-agent delegation patterns
ADK has two ways an agent can delegate to another within a turn. `AgentTool(agent=sub_agent)` wraps any agent as a callable tool — the LLM invokes it like a function and gets the result back. `transfer_to_agent(agent_name)` is an LLM-driven handoff — the model emits this function call and the framework routes the conversation to the named agent. trogonai has `ctx.spawn_agent()` which is fire-and-wait, but it is not surfaced as a `ToolDef` and cannot be called mid-turn by the model as a regular tool.

### A2A Protocol
ADK implements [A2A](https://a2a-protocol.org) — the open standard for agents calling each other over HTTP across processes and vendors. trogonai uses ACP over NATS, which is a different standard and incompatible with the A2A ecosystem.

### Tool system
ADK derives JSON Schema automatically from function type hints / Zod / struct tags — zero boilerplate. trogonai requires hand-written `serde_json::Value` schemas and manual dispatch blocks. ADK also has: `OpenAPIToolset` (auto-generates tools from an OpenAPI spec), `MCPToolset` (stdio + HTTP, auto-discovers tools), `google_search` and `built_in_code_execution` as built-in tools, `LongRunningFunctionTool` for async polling, and 70+ pre-built integration toolsets (BigQuery, Stripe, GitHub, Slack, Notion, Pinecone, etc.). trogonai has none of these.

### ToolContext — what tools can do
Every tool function in ADK receives a `ToolContext` that gives it: read/write access to session state (`tool_context.state["key"]`), artifact save/load, cross-session memory search (`tool_context.search_memory(query)`), in-band OAuth initiation (`tool_context.request_credential()`), and loop control (`tool_context.actions.escalate = True`). trogonai's `ToolContext` only carries `http_client` and `proxy_url` — no state, no memory, no artifacts, no auth flow, no loop control.

### Tool authentication
ADK has an in-band OAuth flow: a tool can call `tool_context.request_credential(AuthConfig)`, the framework pauses execution, the client completes the browser redirect, and the agent resumes with the token. Supports API key, HTTP Bearer, OAuth2, OIDC, and service account. trogonai has no in-band auth — credentials are pre-provisioned in the vault.

### Session state and memory
ADK's state is a dict accessible inside every tool and callback, with scoped prefixes (`user:`, `app:`, `temp:`), instruction template injection (`{state_key}`), and `output_key` for auto-saving agent output. trogonai has no in-turn state dict for tools. ADK's `MemoryService` provides cross-session semantic retrieval (`search_memory(query)`) backed by vector search in production. trogonai stores all history in JetStream but has no retrieval layer — no `MemoryService`, no semantic search.

### Structured output
ADK's `output_schema` forces the model to return valid JSON matching a Pydantic model. `output_key` automatically saves the result to `session.state["key"]`, making it available to downstream agents via `{key}` template interpolation in their instructions. trogonai returns raw strings — structured output requires prompt-engineering and manual parsing by the caller.

### Artifacts
ADK has a versioned binary artifact store accessible in tools: `save_artifact()`, `load_artifact(version)`, `list_artifacts()`. Session-scoped and user-scoped. Backends: in-memory (dev) or GCS (prod). trogonai has no artifact concept.

### Callbacks
ADK has 6 hook points on every agent: `before_agent`, `after_agent`, `before_model`, `after_model`, `before_tool`, `after_tool`. Each can short-circuit by returning a value, and each has read/write access to session state. trogonai has one hook — `PermissionChecker` — which can only allow or deny a tool call.

### Evaluation ← biggest gap
ADK ships a complete eval loop: `.test.json` and `.evalset.json` test formats, 11 built-in metrics (`tool_trajectory_avg_score`, `response_match_score`, `final_response_match_v2`, `hallucinations_v1`, `safety_v1`, `multi_turn_task_success_v1`, and more), `adk eval` CLI, pytest integration, and an interactive web UI. trogonai has zero evaluation tooling. `trogon-transcript` has the raw data — nothing processes it.

### Streaming
ADK supports bidirectional real-time streaming via the Gemini Live API (`run_live`, `LiveRequestQueue`, `RunConfig`) with text, audio, and video modalities. trogonai streams text, tool calls, and thinking events. No audio or video.

### Deployment and REST API
`adk deploy agent_engine` packages and deploys to Vertex AI in one command. `adk api_server` exposes `POST /run`, `POST /run_sse`, and session CRUD instantly. trogonai has none of this — agents are NATS subscribers, deployment is manual.

### Observability
Vertex AI Agent Platform has a dedicated Observe tier: an agent topology view that maps every agent-to-agent call graphically, a managed trace viewer showing per-turn latency and tool calls, Cloud Logging integration for all agent events, and Cloud Monitoring dashboards with alerting. trogonai records raw transcripts in JetStream via `trogon-transcript` but has no trace viewer, no topology visualization, no cloud logging integration, and no metrics surface.

### Managed sessions (platform-level)
Beyond the `SessionService` API in ADK core, Vertex AI exposes Sessions as a first-class platform resource: create/list/get/delete via REST or the Vertex AI console, IAM Conditions for per-session access control, and cross-agent session sharing without any code change. trogonai's session state lives inside ACP-runner JetStream subjects with no platform-level management, no console UI, and no access-control model.

### Vertex AI platform services
Beyond ADK core, the platform adds:

- **Memory Bank** — LLM-extracted per-user memory profiles (not just raw session storage). Includes event ingestion, memory generation, memory profiles, memory revisions, and fetch API.
- **RAG Engine** — full document ingestion pipeline: PDF/HTML/DOCX parsing (including Document AI layout parser and LLM parser), chunking, embedding, and retrieval across multiple vector DB backends (RagManagedDb, Vector Search 2.0, Weaviate, Pinecone, Feature Store, Agent Platform Search). Includes reranking.
- **Vector Search 2.0** — managed vector index with hybrid search (vector + keyword), Private Service Connect, JWT auth.
- **Agent Gateway** — traffic routing between deployed agents, authorization delegation across agent boundaries, monitoring.
- **Model Armor + Semantic Governance** — content security layer on all agent I/O, policy engine for defining and testing allowed/denied agent behaviors.
- **Agent Identity + Agent Registry** — GCP service account per deployed agent, catalog of all deployed agents, cross-project sharing.
- **Agent Studio** — visual agent builder and trace viewer in the Vertex AI console.
- **Production Evaluation Service** — cloud-side eval beyond local `adk eval`: online continuous monitors on live traffic, quality alerts, failure clustering, results analysis dashboard, prompt optimization pipeline.
- **Example Store** — managed few-shot example library: upload, retrieve, and manage examples for prompt improvement.
- **Code Execution sandbox** — managed serverless sandbox for agent-generated code (separate from the local `BuiltInCodeExecutor`).

None of these exist in trogonai.

---

## Where trogonai leads

| trogonai capability | ADK equivalent |
|---|---|
| 11 webhook sources with signature validation | Not in ADK — starts at the agent |
| Encrypted vault (Argon2id + AES-256-GCM), opaque tokens, approval workflows | Basic auth primitives |
| Human-in-the-loop approval workflows (`trogon-vault-approvals`) | None |
| Distributed CRON with leader election | None |
| Entity actor model (stateful per domain entity, OCC) | Not first-class |
| At-least-once delivery via NATS JetStream | HTTP is at-most-once |

---

## Priority gaps to close

| # | Gap | Why it matters |
|---|---|---|
| 1 | **Evaluation framework** | Can't measure quality, detect regressions, or iterate on prompts confidently |
| 2 | **Cross-session MemoryService** | Agents forget everything between sessions |
| 3 | **Workflow composition** (Sequential / Parallel / Loop) | No composable pipelines in code |
| 4 | **Callback system** | Can't inject guardrails, caching, or context without forking the loop |
| 5 | **In-turn state dict + scoped prefixes** | Tools can't share data within a session |
| 6 | **Structured output** (`output_schema`, `output_key`) | Agent output is untyped raw text |
| 7 | **Tool schema derivation** | Hand-writing JSON Schema for every tool is error-prone boilerplate |
| 8 | **OpenAPI tool generation** | Every external REST API requires custom code |
| 9 | **Artifact store** | No place to put files agents produce |
| 10 | **REST serving layer** | No standard HTTP surface for external callers |
| 11 | **Model abstraction** | Switching from Claude to Gemini/Ollama requires a different crate |
| 12 | **A2A Protocol** | Isolated from the Google agent ecosystem |
| 13 | **Observability** (traces, topology, Cloud Logging/Monitoring) | No visibility into multi-agent call graphs or per-turn latency |
| 14 | **Planner / global_instruction** | No structured plan-before-act, no hierarchy-wide instruction injection |
