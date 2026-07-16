# Google ADK + A2A protocol: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence snapshot retrieved 2026-07-13. Version-sensitive claims were checked
against these authoritative anchors:

- ADK Python 2.4.0 at commit
  [`3f79243`](https://github.com/google/adk-python/tree/3f7924388361522a5d1b5e0ca1adb4d33c38abf8):
  [`BaseAgent`](https://github.com/google/adk-python/blob/3f7924388361522a5d1b5e0ca1adb4d33c38abf8/src/google/adk/agents/base_agent.py#L93-L147),
  [`LlmAgent` and `Agent`](https://github.com/google/adk-python/blob/3f7924388361522a5d1b5e0ca1adb4d33c38abf8/src/google/adk/agents/llm_agent.py#L223-L398),
  and the [`Agent` alias](https://github.com/google/adk-python/blob/3f7924388361522a5d1b5e0ca1adb4d33c38abf8/src/google/adk/agents/llm_agent.py#L1252).
- ADK documentation,
  [Agents](https://google.github.io/adk-docs/agents/),
  [Multi-agent systems](https://google.github.io/adk-docs/agents/multi-agents/),
  [Runtime](https://google.github.io/adk-docs/runtime/), and
  [Sessions](https://google.github.io/adk-docs/sessions/).
- ADK Python 2.4.0 workflow sources: the
  [`SequentialAgent`](https://github.com/google/adk-python/blob/3f7924388361522a5d1b5e0ca1adb4d33c38abf8/src/google/adk/agents/sequential_agent.py#L49-L62),
  [`ParallelAgent`](https://github.com/google/adk-python/blob/3f7924388361522a5d1b5e0ca1adb4d33c38abf8/src/google/adk/agents/parallel_agent.py#L160-L176),
  and [`LoopAgent`](https://github.com/google/adk-python/blob/3f7924388361522a5d1b5e0ca1adb4d33c38abf8/src/google/adk/agents/loop_agent.py#L53-L66)
  deprecations, plus the replacement
  [`Workflow`](https://github.com/google/adk-python/blob/3f7924388361522a5d1b5e0ca1adb4d33c38abf8/src/google/adk/workflow/_workflow.py#L15-L19).
- A2A protocol 1.0.0, specifically specification sections
  [2.2, 3.4, 8, and Appendix B](https://a2a-protocol.org/v1.0.0/specification/),
  and the normative [`a2a.proto`](https://github.com/a2aproject/A2A/blob/3303592588e388e62e0f69f701af531d2f4e3991/specification/a2a.proto#L361-L395)
  pinned at the A2A 1.0.1 repository release commit.
- Vertex AI Agent Engine
  [overview](https://cloud.google.com/vertex-ai/generative-ai/docs/agent-engine/overview),
  REST v1 [`ReasoningEngineSpec`](https://docs.cloud.google.com/gemini-enterprise-agent-platform/reference/rest/v1/projects.locations.reasoningEngines#ReasoningEngineSpec),
  and v1beta1 [revisions and traffic](https://docs.cloud.google.com/gemini-enterprise-agent-platform/scale/runtime/manage-revisions-and-traffic),
  which lists `agentCard[]` among versioned fields.

Three layers, three models: ADK (code object), A2A (network endpoint +
manifest), Vertex Agent Engine (cloud resource).

## The `agent` noun (primary-source quotes)

- **ADK:** "An **Agent**, or **LlmAgent**, in ADK is a self-contained
  execution unit designed to act autonomously to achieve specific goals...
  The basic components of an Agent are an AI model, task instructions, and
  optionally, a set of tools." In code, `BaseAgent(BaseNode)` is a pydantic
  object (`name`, `description`, `parent_agent`, `sub_agents`, callbacks);
  `Agent` is literally `TypeAlias = LlmAgent`. Any custom class subclassing
  `BaseAgent` with `_run_async_impl` is an agent. Same model across
  Python/TS/Go/Java.
- **A2A:** "An open protocol enabling communication and interoperability
  between opaque agentic applications." "Agents are autonomous
  problem-solvers that act independently within their environment," and
  pointedly: "encapsulating an agent as a simple tool is fundamentally
  limiting, as it fails to capture the agent's full capabilities."
- **The AgentCard is the interoperable definition:** "A self-describing
  manifest for an agent. It provides essential metadata including the
  agent's identity, capabilities, skills, supported communication methods,
  and security requirements." (a2a.proto) Required fields: `name`,
  `description`, `supported_interfaces` (url + protocol binding JSONRPC/
  GRPC/HTTP+JSON + protocol version), `version`, `capabilities` (streaming,
  push_notifications, extended card), `default_input/output_modes`,
  `skills` (id/name/description/examples); plus security schemes
  (api-key/HTTP/OAuth2/OIDC/mTLS), multi-tenant routing (`tenant`), and JWS
  `signatures` for card integrity. Discovered at
  `/.well-known/agent-card.json` (RFC 8615), cacheable via HTTP headers,
  with an authenticated extended card for sensitive detail.
- **Vertex Agent Engine:** the deployed resource is a `ReasoningEngine`
  ("a customizable runtime for models to determine which actions to take
  and in which order"). Its v1 `ReasoningEngineSpec` declares the framework
  in `agentFramework` (`google-adk`, `langchain`, ...); the v1beta1 revision
  surface lists A2A Agent Cards in `agentCard[]` as a versioned field.
- **Conceptual models:** ADK = agent-as-code-object in a tree; A2A =
  agent-as-network-endpoint defined entirely by its declared manifest;
  Agent Engine = agent-as-cloud-resource bridging the two.

## Subagents

- **ADK's `sub_agents` is a first-class tree:** parent set automatically;
  "an agent instance can only be added as a sub-agent once" (single parent,
  tree not DAG; no documented depth limit).
- **Delegation is LLM-driven via description:** "This description is
  primarily used by *other* LLM agents to determine if they should route a
  task to this agent"; the model emits
  `transfer_to_agent(agent_name=...)`, or ADK "automatically generate[s] a
  delegation tool for each subagent, named after the subagent itself."
  Transfer can be constrained (`disallow_transfer_to_parent/peers`).
- **Agent modes govern return-of-control:** `chat` ("manual return to
  parent"), `task` ("automatic return... must be leaf agents"), and
  `single_turn` ("no user interaction... can be run in parallel").
- **Workflow agents are deterministic orchestrators:** SequentialAgent
  "passes the same InvocationContext to each of its sub-agents" (shared
  state including `temp:`); ParallelAgent gives "each sub-agent... its own
  execution branch... **no automatic sharing of conversation history or
  state between these branches**"; LoopAgent iterates until escalation or
  `max_iterations`. **All three are formally deprecated** (see the
  dedicated section below).
- **AgentTool** wraps an agent as a tool: runs it in an isolated Runner,
  forwards state/artifact deltas back, returns the final response as the
  tool result.
- **A2A inverts the hierarchy entirely: peers, not parents.** Any agent can
  be client and server; the remote agent is opaque ("Agents collaborate
  based on declared capabilities and exchanged information, without needing
  to share their internal thoughts, plans, or tool implementations"). The
  unit of delegation is the **Task**, not the agent.

## The workflow-agent deprecation (verified in source, 2026-07-08)

Google shipped deterministic orchestrators *as agent subclasses* and then
walked it back. All three carry a runtime `@deprecated` decorator in
`adk-python` (`src/google/adk/agents/{sequential,parallel,loop}_agent.py`),
verified verbatim:

> "SequentialAgent is deprecated in favor of Workflow and will be removed
> in a future version. Workflow cannot yet be used as an LlmAgent
> sub-agent."

(ParallelAgent and LoopAgent carry the identical notice with their names;
the YAML `AgentConfig` loader and per-class `config_type` are deprecated
alongside them.)

The replacement is a separate `google.adk.workflow` package, **not an
agent subclass**: "New Workflow implementation, BaseNode with graph
orchestration. Combines user-facing graph definition with the execution
engine. Workflow(BaseNode) with `_run_impl()` as the orchestration loop."
(`workflow/_workflow.py`). The module ships a full graph engine: `_graph`
(edges), node kinds (`_function_node`, `_tool_node`, `_join_node`,
`_parallel_worker`, and notably `_llm_agent_wrapper`, the LLM agent
becomes *a node inside the workflow*, inverting the old
containment), a `DynamicNodeScheduler`, `Trigger`s, retry config, and
replay/rehydration utilities (`_replay_interceptor`,
`_rehydration_utils`, durable re-execution semantics).

Why this matters for the study: it is the clearest industry admission that
**deterministic orchestration dressed as an agent is a category error**.
The migration inverts the relationship: before, workflow-agents contained
LLM agents as sub_agents; after, a Workflow graph contains an LLM agent as
one wrapped node among function/tool nodes. The transitional caveat
("Workflow cannot yet be used as an LlmAgent sub-agent") shows the seam
mid-migration. This is the primary evidence for our design rule (service
design Q12): determinism belongs in tools, runtimes, or external
workflows, never in the agent noun.

## Configuration surface (what, where, why)

- **ADK LlmAgent fields** (with doc rationales): `name` ("crucial... where
  agents need to refer to or delegate tasks to each other"), `description`
  (routing signal), `model` (inherits from ancestor if empty; "impacts
  capabilities, cost, and performance"), `instruction` ("arguably the most
  critical... core task, persona, constraints, how and when to use tools,
  output format"; supports `{placeholders}` and provider functions),
  `static_instruction` (context-caching prefix), `global_instruction`
  (root-wide, deprecated), `tools` (functions, BaseTools, or AgentTools),
  `generate_content_config`, `input_schema`/`output_schema` (structured
  I/O), `output_key` ("final response... automatically saved to the
  session's state dictionary"), `include_contents` (`'none'` = no history),
  `planner`, `code_executor`, `mode`, transfer switches, and six
  model/tool/error callbacks. Config is Python constructor calls only (the
  YAML config layer is deprecated).
- **A2A AgentCard is declared capability, not behavior config:** it states
  what the agent can do, where to reach it, and how to authenticate: "it
  does not configure internal behavior, that remains opaque."
- **Agent Engine adds ops config:** container/source spec, framework tag,
  Agent Cards, and revision traffic splitting.

## Binding time

- **ADK: construction time.** Pydantic `model_post_init` bakes mode-derived
  tools; parents bind when `sub_agents` is assigned; no late binding.
  Instruction *values* can late-bind via provider functions and state
  placeholders per invocation.
- **A2A: discovery time.** The card is fetched at
  `/.well-known/agent-card.json`; staleness is governed by HTTP
  `Cache-Control`; JWS signatures make cards verifiable; the authenticated
  `GetExtendedAgentCard` RPC gates the sensitive layer.
- **Agent Engine: deploy time with revisions.** `ReasoningEngineRuntimeRevision`
  is "a specific version of the runtime related part"; traffic config
  chooses `trafficSplitManual` (percentage per revision) or
  `trafficSplitAlwaysLatest`, the AWS/OpenComputer-style version/endpoint
  split, expressed as traffic management.

## Relationships between nouns

- **ADK:** Runner (binds agent + session/artifact/memory/credential
  services) 1:N sessions; session = "a single, ongoing interaction...
  chronological sequence of Events"; invocation = "everything that happens
  in response to a *single* user query" (one `invocation_id`, possibly many
  agent runs); events carry `state_delta` actions. State prefixes scope
  persistence: none (session), `user:` (cross-session per user), `app:`
  (cross-user), `temp:` (invocation-only). Memory is the cross-session
  searchable store; artifacts are saved outputs.
- **A2A:** "**Task is the core unit of action for A2A**. It has a current
  status and when results are created... they are stored in the artifact.
  If there are multiple turns... these are stored in history." `contextId`
  "logically groups multiple Task objects and independent Message objects,
  providing continuity", 1 agent : N tasks; 1 contextId : N tasks/messages;
  1 task : N artifacts/messages; messages contain Parts (text/raw/url/data).
  Three interaction styles: message-only agents, task-generating agents,
  hybrid agents (messages to negotiate scope, tasks to track execution).
- **Bridge:** an ADK agent deploys as a ReasoningEngine; the preview revision
  surface can carry Agent Cards: code object → cloud resource → discoverable
  endpoint.

## Lifecycle

- **ADK invocation loop (cooperative, event-sourced):** Runner receives the
  query → agent yields Events → Runner commits actions (state deltas) →
  agent resumes "only *after* the Runner has processed the event." State
  changes are "only **guaranteed to be persisted** after the Event carrying
  the corresponding state_delta has been yielded... and processed." Partial
  streaming events skip action processing; only the final event commits.
  Customer code owns the process locally; Agent Engine wraps the same via
  `:query`/`:streamQuery`/`:asyncQuery`.
- **A2A Task state machine** (proto): `SUBMITTED`, `WORKING` (active);
  `COMPLETED`, `FAILED`, `CANCELED`, `REJECTED` (terminal);
  `INPUT_REQUIRED`, `AUTH_REQUIRED` (interrupted). **Terminal tasks never
  restart:** "Any subsequent interaction related to that task, such as a
  refinement, must initiate a new task within the same contextId", for
  task immutability, "a clean mapping of inputs to outputs," and tracing
  "each artifact to a specific unit of work."
- **Delivery:** polling (`GetTask`), SSE streaming (`SendStreamingMessage`,
  stream closes on terminal/interrupted state, `SubscribeToTask` to
  reattach), and push notifications to registered webhooks.

## What makes it "an agent" here (our inference)

Our inference: Google's stack answers the definition question at two
different layers on purpose. Inside a process (ADK), an agent is a named
node in a delegation tree, a code object whose `description` is its
routing contract. Between organizations (A2A), an agent is **whatever
stands behind an endpoint and honors its card**: identity + declared
skills + protocol bindings, with internals explicitly opaque, and the
*task*, not the agent, as the durable, immutable unit of work. The
A2A/MCP line is the industry's crispest boundary statement: "A2A is about
agents *partnering* on tasks, while MCP is more about agents *using*
capabilities", tools are stateless primitives; agents reason, plan,
maintain state, and negotiate. For a service design, A2A's AgentCard is
the reference schema for agent *identity*, and its task immutability rule
is the reference semantics for agent *work*.

## Open questions

- ADK 2.0's `Workflow` internals are now partially read from source (see
  the deprecation section); its public docs and the resolution of "Workflow
  cannot yet be used as an LlmAgent sub-agent" remain to watch.
- The normative AgentCard JSON Schema is deliberately a build artifact of
  the proto, consumers must track the proto.
- AGNTCY/Agent Directory (cross-vendor discovery beyond well-known URIs)
  did not appear in the spec pages read.
- Extended AgentCard field differences from the public card are not
  documented in the fetched pages.
