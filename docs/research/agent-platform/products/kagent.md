# kagent: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence from kagent.dev docs and the `kagent-dev/kagent` source (CRD Go
types, controller/translator, ADK runtimes, in-repo architecture docs).
kagent is a CNCF sandbox framework for running AI agents on Kubernetes.

## Source anchors

Sources retrieved 2026-07-13. Source-level claims are pinned to the
[kagent v0.9.11 release](https://github.com/kagent-dev/kagent/releases/tag/v0.9.11)
(2026-07-01) at commit
[`14dcbfc`](https://github.com/kagent-dev/kagent/commit/14dcbfc49bde990a0c737da14a9d94cf7e788303).
The kagent.dev docs pages are unversioned live pages built from a separate
`kagent-dev/website` repository, so doc quotes are not release-pinned; where
docs and pinned source could diverge, this dossier prefers the pinned source.

- Product concepts: [Agents](https://kagent.dev/docs/kagent/concepts/agents),
  [Tools](https://kagent.dev/docs/kagent/concepts/tools),
  [Architecture](https://kagent.dev/docs/kagent/concepts/architecture),
  [Agent Memory](https://kagent.dev/docs/kagent/concepts/agent-memory),
  [Agent Substrate](https://kagent.dev/docs/kagent/concepts/agent-substrate),
  [Agent Harness](https://kagent.dev/docs/kagent/concepts/agent-harness), and
  [What is kagent](https://kagent.dev/docs/kagent/introduction/what-is-kagent).
- Reference: [API reference](https://kagent.dev/docs/kagent/resources/api-ref),
  [FAQ](https://kagent.dev/docs/kagent/resources/faq),
  [A2A example](https://kagent.dev/docs/kagent/examples/a2a-agents), and
  [operational considerations](https://kagent.dev/docs/kagent/operations/operational-considerations).
- Stable CRD symbols: [`Agent`, `AgentSpec`, `Tool`, `TypedReference`,
  `A2AConfig`, `AgentStatus`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/v1alpha2/agent_types.go),
  [`ModelConfig`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/v1alpha2/modelconfig_types.go),
  and [database models](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/database/models.go).
- Stable runtime symbols: [agent-as-tool compiler](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/core/internal/controller/translator/agent/compiler.go),
  [reconciler](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/core/internal/controller/reconciler/reconciler.go),
  [remote A2A tool](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/adk/pkg/tools/remote_a2a_tool.go),
  and [in-repo architecture docs](https://github.com/kagent-dev/kagent/tree/14dcbfc49bde990a0c737da14a9d94cf7e788303/docs/architecture).

## The `agent` noun (primary-source quotes)

- [Concepts, Agents](https://kagent.dev/docs/kagent/concepts/agents): "An AI
  agent is an application that can interact with users in natural language.
  Agents use LLMs to generate responses to user queries and can also execute
  actions on behalf of the user." The same page decomposes it into exactly
  three components: instructions ("A set of instructions that define the
  agent's behavior and capabilities. This is also called a system prompt"),
  "Tools: Functions that the agent can use to interact with its environment,"
  and "Skills: Descriptions of capabilities that help the agent act more
  autonomously."
- **Operationally the agent is a Kubernetes custom resource**: kind `Agent`,
  group `kagent.dev`, namespaced, current storage version `v1alpha2`
  (v1alpha1 is retained unserved). "Agent is the Schema for the agents API"
  ([`agent_types.go:664-674`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/v1alpha2/agent_types.go#L664-L674);
  [CRD manifest](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/config/crd/bases/kagent.dev_agents.yaml#L1-L17)).
  A persistent, identified, GitOps-able record, not a run.
- **The spec bifurcates the noun into two agent kinds**
  ([`AgentType` enum](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/v1alpha2/agent_types.go#L31-L38)):
  "Declarative configures an agent that is fully described by this resource
  (model, instructions, tools) and runs on one of kagent's built-in runtimes,"
  versus BYO: "a 'bring your own' agent backed by a user-provided container
  image. Kagent deploys the image and expects it to serve the agent over the
  A2A protocol on port 8080"
  ([`agent_types.go:58-68`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/v1alpha2/agent_types.go#L58-L68)).
- Two sibling CRDs extend the noun: **SandboxAgent** ("declares an agent that
  runs in an isolated sandbox on Agent Substrate", per the API reference) and
  **AgentHarness**, which "provisions the execution environment itself and
  runs a third-party coding agent inside it" (backend enum:
  `openclaw;nemoclaw;hermes`,
  [`agentharness_types.go:19-21`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/v1alpha2/agentharness_types.go#L19-L21)),
  explicitly distinguished from `Agent`, which "runs a kagent-managed agent
  runtime" ([Agent Harness](https://kagent.dev/docs/kagent/concepts/agent-harness)).
- **Conceptual model: agent-as-config reconciled into
  agent-as-deployed-service.** The CRD spec is the complete definition
  (config); the controller continuously materializes it into a running
  A2A-speaking workload (one Deployment per agent, or a substrate actor).
  Execution state lives in separate Session/Task/Memory records scoped to the
  agent's identity.

## Subagents

- **Agent-as-tool, declared in the CRD roster.** The `Tool` union type on a
  Declarative agent has exactly two provider kinds: `McpServer` and `Agent`
  ([`ToolProviderType`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/v1alpha2/agent_types.go#L488-L516)).
  Docs: "You also have an option of using agents as tools. Any agent you
  create can be referenced and used by other agents you have. You can refer
  to agents in other namespaces by using the `namespace/name` format"
  ([Tools, "Agents as Tools"](https://kagent.dev/docs/kagent/concepts/tools)).
  The worked example is a PromQL agent used as a tool by a second agent
  "whenever it needs to create a PromQL query"
  ([Agents, "Agents as Tools"](https://kagent.dev/docs/kagent/concepts/agents)).
- **The subagent is not spawned; it is referenced.** The `agent` field is a
  bare
  [`TypedReference`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/v1alpha2/agent_types.go#L578-L587)
  (kind/apiGroup/name/namespace). The referenced agent is an independently
  deployed workload with its own Service; the compiler resolves the reference
  to a URL (`http://{name}.{namespace}:8080`, or a controller-proxied path for
  sandbox agents,
  [`toolAgentURL`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/core/internal/controller/translator/agent/compiler.go#L86-L99)).
- **Inheritance is nothing; the contract is identity-only.** The parent's
  compiled config carries per child only
  [`RemoteAgentConfig{Name, Url, Headers, Description}`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/adk/types.go#L377-L382);
  no model, tools, memory, or approval lists cross the boundary, in either
  the Go or the Python runtime. What does cross: the end user's identity
  (`x-user-id` header) and conversation lineage headers
  (`x-kagent-parent-context-id`, `x-kagent-root-context-id`), plus a static
  `x-kagent-source: agent` marker that "hides the session from the agent's
  session history sidebar"
  ([`remote_a2a_tool.go`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/adk/pkg/tools/remote_a2a_tool.go#L26-L63);
  [a2a-subagents.md](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/docs/architecture/a2a-subagents.md)).
- **Fresh child session per delegation edge.** Each remote-agent tool mints
  its own A2A context ID at construction, independent of the parent's session:
  "KAgentRemoteA2ATool.__init__ generates a UUID (`_last_context_id`) that is
  used as the A2A `context_id` for every message sent to the subagent. On the
  subagent side, this `context_id` becomes the session ID"
  ([a2a-subagents.md:13](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/docs/architecture/a2a-subagents.md#L13)).
- **Communication is a single blocking A2A `SendMessage`** (JSON-RPC), results
  only; child failures come back as an error string in the tool output rather
  than aborting the parent's turn, and HITL pauses tunnel through a two-phase
  resume protocol
  ([`remote_a2a_tool.go:255-272`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/adk/pkg/tools/remote_a2a_tool.go#L255-L272)).
- **Nesting: a DAG with depth 10.** Reconcile-time validation recurses the
  tool chain: "cycle detected in agent tool chain," "recursion limit reached
  in agent tool chain," and "agent tool cannot be used to reference itself"
  errors, bounded by `const MAX_DEPTH = 10` (the chain may nest up to 10
  levels)
  ([`compiler.go:23`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/core/internal/controller/translator/agent/compiler.go#L23),
  [`:174-180`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/core/internal/controller/translator/agent/compiler.go#L174-L180),
  [`:199-201`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/core/internal/controller/translator/agent/compiler.go#L199-L201)).
- **Lifetime coupling: none, and nothing protects the edge.** No finalizer,
  webhook, or reference count prevents deleting an agent that others use as a
  tool; the parent's `Accepted` condition flips to False (reason
  `ReconcileFailed`) on its next reconcile while its `Ready` condition,
  computed only from its own Deployment, can stay stale-True
  ([`reconciler.go:190-193`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/core/internal/controller/reconciler/reconciler.go#L190-L193),
  [`:287-299`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/core/internal/controller/reconciler/reconciler.go#L287-L299)).
  Cross-namespace referencing is gated by `allowedNamespaces`, which "follows
  the Gateway API pattern for cross-namespace route attachments"
  ([`agent_types.go:84-90`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/v1alpha2/agent_types.go#L84-L90)).

## Configuration surface (what, where, why)

- **Declarative agent fields**
  ([`DeclarativeAgentSpec`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/v1alpha2/agent_types.go#L168-L231)):
  `runtime` (`python` ADK, default, or `go` ADK), `systemMessage` /
  `systemMessageFrom` (mutually exclusive by CEL rule) with Go
  `promptTemplate` composition, `modelConfig` (by-name reference, default
  `"default-model-config"`), `tools` (max 20), `a2aConfig` (published
  `AgentSkill` list), `memory` (`modelConfig` embedder reference plus
  `ttlDays`, default 15), `stream`, `executeCodeBlocks`, and `deployment`.
  Agent-level extras: `skills` (OCI images or Git repos, "made available to
  the agent under the `/skills` folder") and `allowedNamespaces`.
- **The environment knob is the Kubernetes pod surface**:
  [`SharedDeploymentSpec`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/v1alpha2/agent_types.go#L422-L477)
  exposes replicas (default 1), resources, env, volumes, node placement,
  security contexts, service accounts, and `extraContainers` ("Useful for
  sidecars such as token proxies, log shippers, or security agents").
- **Model and tools are separate CRDs referenced by name.**
  [`ModelConfig`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/v1alpha2/modelconfig_types.go#L29-L31)
  enumerates nine providers (Anthropic, OpenAI, AzureOpenAI, Ollama, Gemini,
  GeminiVertexAI, AnthropicVertexAI, Bedrock, SAPAICore) with per-provider
  blocks gated by CEL rules; `RemoteMCPServer` (v1alpha2) carries MCP
  endpoint, protocol, and header config. Credentials are always Secret
  references (`apiKeySecret`, `headersFrom`), never inline values.
- **Permissions are per-tool HITL**: `requireApproval` lists tool names that
  pause for human approval, CEL-enforced to be a subset of `toolNames`
  ([`agent_types.go:533`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/v1alpha2/agent_types.go#L533)).
  Docs: "Tools listed in `requireApproval` pause execution and present
  Approve/Reject buttons in the UI."
- **Where config lives**: CRD YAML via `kubectl` is canonical; Helm values
  install the platform and bundled agents; the dashboard UI has a
  create/edit wizard; the CLI manages resources and invokes agents. No
  triggers: there is no cron or webhook field on `Agent`; the only
  trigger-adjacent surface is `AgentHarness.spec.channels` (Slack/Telegram
  bindings for harness-run coding agents).
- **Stated rationale, quoted:** declarativeness is the product thesis:
  "kagent is designed from the ground up to be declarative. You define the
  agents, tools, and instructions and kagent will take care of the rest. Most
  other frameworks are procedural and require you to write code to tell the
  LLM what to do" ([FAQ](https://kagent.dev/docs/kagent/resources/faq)).
  CRDs exist so you can "Define, version, and roll out agents with kubectl
  and GitOps"
  ([What is kagent](https://kagent.dev/docs/kagent/introduction/what-is-kagent)).
  Prompt templates take ConfigMaps only: "Secret references are intentionally
  excluded to avoid leaking sensitive data into prompts sent to LLM
  providers." HITL exists "to keep humans in control of agent actions."
  Cross-namespace references let you "share tool servers across namespaces
  without duplicating them"
  ([Agents](https://kagent.dev/docs/kagent/concepts/agents);
  [Tools](https://kagent.dev/docs/kagent/concepts/tools)).
- **The protocol choices are asserted, not argued.** The A2A feature bullet
  says only "Agents discover and invoke each other. Compose multi-agent
  workflows with first-class delegation"
  ([features](https://kagent.dev/docs/kagent/introduction/features)); MCP is
  justified by traction: "a protocol, originally created by Anthropic, which
  is meant as a flexible way to provide tools and other information to
  Agents... it has begun to gain traction and more and more tools are
  adopting it" ([Tools](https://kagent.dev/docs/kagent/concepts/tools)). The
  interoperability rationale exists only as an outbound link to Google's A2A
  announcement, never restated in kagent's own prose.

## Binding time

- **Reconcile time is the binding moment.** The controller resolves
  ModelConfig, prompt templates, and MCP references into one fully resolved
  `config.json` baked into a Secret before the pod starts: "Prompt templates
  are resolved by the controller, not at runtime. The agent receives a fully
  resolved string. This makes debugging easier and keeps the runtime simple"
  ([architecture README](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/docs/architecture/README.md)).
- **Rebinding means a pod roll, not a mutation.** The pod reads `config.json`
  once at startup; a config-hash annotation on the pod template forces a
  rolling update when the serialized config or referenced Secrets change
  ([`manifest_builder.go:27-30`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/core/internal/controller/translator/agent/manifest_builder.go#L27-L30)).
  Docs make secret rotation the sanctioned live path: "kagent automatically
  restarts agents when you update the secrets that the agents reference"
  ([operational considerations](https://kagent.dev/docs/kagent/operations/operational-considerations)).
- **Per-request bindings**: session identity binds at invocation
  (get-or-create by `app_name`, `user_id`, `session_id` in the ADK executor),
  and each HITL approval decision binds per tool call at runtime.
- **Versioning is Kubernetes versioning.** Nothing beyond
  `generation`/`resourceVersion` plus GitOps; the `version` spec field is
  only "the agent's version string, surfaced on the A2A AgentCard" (API
  reference). The platform database guarantees an n-1 rollback window
  ([upgrade guide](https://kagent.dev/docs/kagent/operations/upgrade)).

## Relationships between nouns

- **CRD kinds (v1alpha2)**: Agent, AgentHarness, ModelConfig,
  ModelProviderConfig, RemoteMCPServer, SandboxAgent. ToolServer and Memory
  survive as v1alpha1-only leftovers never promoted. There is **no Session
  CRD and no Team kind**; Team was an AutoGen-era concept that disappeared
  with the v0.6 ADK migration (release notes: "The `apiVersion` field in the
  kagent CRDs is now `kagent.dev/v1alpha2`"). The intro page still promises
  that agents "can also be grouped into teams where a planning agent comes
  up with a plan and assigns tasks to individual agents in the team," but no
  CRD, controller path, or docs page backs it at the pinned commit;
  agent-as-tool is the only implemented composition.
- **Agent 1:1 ModelConfig (by name), 1:N tools (max 20), each tool an MCP
  server or another Agent.** Agent-as-tool edges make the agent graph
  many-to-many, validated as a DAG.
- **Session is a database row, not a resource**:
  `Session{ID, Name, UserID, AgentID, Source}` in the controller's
  SQLite/Postgres store, where `Source` distinguishes `user` from `agent`
  ("created by a parent agent's A2A call",
  [`models.go:55-77`](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/go/api/database/models.go#L55-L77)).
  One agent, many sessions. **Task** is the A2A unit of work, N:1 to Session.
  **Memory** rows are agent-plus-user scoped vectors with TTL: "Each agent
  has its own isolated memory store. You cannot share memories across
  agents," and memory is "built on the Google ADK memory implementation and
  cannot be swapped"
  ([Agent Memory](https://kagent.dev/docs/kagent/concepts/agent-memory)).
- **The CRD is the source of truth; the DB is a cache.** "The DB provides
  fast lookups for the HTTP API and UI, while the CRDs remain the source of
  truth for agent configuration"
  ([architecture README](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/docs/architecture/README.md)).
- **Agent implies its own environment**: one Deployment plus Service plus
  config Secret per agent; SandboxAgent swaps that for a substrate actor.
  "Session" is overloaded across subsystems: on Agent Substrate it means "The
  execution context that tracks an actor's activity and checkpoints"
  ([Agent Substrate](https://kagent.dev/docs/kagent/concepts/agent-substrate)).

## Lifecycle

- **Create**: `kubectl apply` fires the reconciler pipeline: translate spec
  into manifests, reconcile owned resources, upsert the DB record, update
  status ([controller-reconciliation.md](https://github.com/kagent-dev/kagent/blob/14dcbfc49bde990a0c737da14a9d94cf7e788303/docs/architecture/controller-reconciliation.md)).
  Status carries `Accepted` (spec compiled) and `Ready` (Deployment replicas
  available), plus `UnsupportedFeatures`.
- **The platform deploys; the pod owns the loop.** "The kagent engine is the
  core component of kagent. It runs the agent's conversation loop and
  supports two runtimes": Python ADK (default, built on Google ADK) and a Go
  ADK ([Architecture](https://kagent.dev/docs/kagent/concepts/architecture)).
  The controller never runs the loop; it proxies: "The controller HTTP server
  proxies A2A requests to agent pods. The UI never talks directly to agent
  pods" (architecture README).
- **Update**: rolling update with `MaxUnavailable=0`, `MaxSurge=1`; the old
  pod drains gracefully on SIGTERM. **Delete**: standard owner-reference
  garbage collection, no finalizer on the base Agent; the controller drops
  the cached DB row when the Get returns NotFound.
- **Pause/resume exists only on Agent Substrate**: "Substrate decouples an
  agent's lifecycle from pod infrastructure. Idle agents are snapshotted to
  object storage and rehydrated on demand, so a small pool of pre-warmed
  workers can host far more agents than there are pods"
  ([Agent Substrate](https://kagent.dev/docs/kagent/concepts/agent-substrate)).
  Regular agents are always-on Deployments.
- **Persists across runs**: sessions, tasks, and events in the controller
  database; memory vectors until TTL expiry; substrate actor snapshots.
- **Invocation surfaces all speak A2A**: the dashboard POSTs A2A JSON-RPC
  through the controller proxy; `kagent invoke` sends the task as an A2A
  call; and "Every AI agent created with kagent implements the A2A protocol
  and can be invoked by an A2A client" at
  `/api/a2a/{namespace}/{agent-name}`, with the agent card at
  `.well-known/agent.json`
  ([A2A example](https://kagent.dev/docs/kagent/examples/a2a-agents)).
  Agents double as tools for outsiders too: "A2A-enabled agents are
  automatically exposed as an MCP server on the kagent controller"
  ([Agents](https://kagent.dev/docs/kagent/concepts/agents)).

## What makes it "an agent" here (our inference)

Our inference: kagent defines an agent **infrastructurally**, as a namespaced
Kubernetes resource whose spec declares persona and capability (instructions,
model reference, tools, skills) and whose reconciler materializes it into a
long-running service that speaks A2A. Agent-ness is not a property of the
loop (that is delegated wholesale to an embedded ADK runtime); it is the
property of being a declaratively managed, addressable workload. The
distinctive datum for the study is that kagent pushes the entire multi-agent
question onto the network: a subagent is just another agent's Service called
over A2A with a fresh session, identity headers forwarded, and nothing else
shared, so delegation inherits Kubernetes semantics (references can dangle,
readiness is per-workload, and isolation is the default) rather than
framework semantics.

## Open questions

- Whether an explicit protocol-choice rationale for A2A/MCP exists outside
  the docs and blog (maintainer talks, the CNCF sandbox proposal) is
  unchecked; on kagent.dev itself the choice is asserted, not argued.
- Why ToolServer and Memory were left behind as v1alpha1-only CRDs while
  their replacements (RemoteMCPServer, inline `memory`) moved to v1alpha2 is
  undocumented.
- What happens to an in-flight multi-turn loop (mid tool-call) during a
  rolling update is unconfirmed: graceful SIGTERM drain is implemented, but
  no source states whether a mid-loop turn survives pod replacement.
- The broken agent-as-tool failure mode (Accepted=False with stale
  Ready=True) is only inferable from source; no docs page describes it.
- The docs site is built from a separate unversioned repository, so doc
  quotes cannot be pinned to the v0.9.11 source snapshot; treat doc-only
  claims as current-site claims.
