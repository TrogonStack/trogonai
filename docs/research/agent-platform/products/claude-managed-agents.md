# Claude Managed Agents: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence from Anthropic's platform docs (beta `managed-agents-2026-04-01`) and
the official API reference skill in `anthropics/skills`.

## Source anchors

Sources retrieved 2026-07-13. Managed Agents was a beta API identified by
`managed-agents-2026-04-01`; no SDK package version is implied by this dossier.

- Product definition and resource lifecycle: [Overview, sections "Core
  concepts" and "Beta access"](https://platform.claude.com/docs/en/managed-agents/overview)
  and [Agent setup, sections "Agent configuration fields", "Update
  semantics", and "Agent lifecycle"](https://platform.claude.com/docs/en/managed-agents/agent-setup).
- Session and environment behavior: [Start a session](https://platform.claude.com/docs/en/managed-agents/sessions),
  [Session operations](https://platform.claude.com/docs/en/managed-agents/session-operations),
  and [Cloud environment setup](https://platform.claude.com/docs/en/managed-agents/environments).
- Delegation and durable services: [Multi-agent sessions](https://platform.claude.com/docs/en/managed-agents/multi-agent),
  [Using agent memory](https://platform.claude.com/docs/en/managed-agents/memory),
  [Scheduled deployments](https://platform.claude.com/docs/en/managed-agents/scheduled-deployments),
  and [Self-hosted sandboxes](https://platform.claude.com/docs/en/managed-agents/self-hosted-sandboxes).
- Skills, credentials, and permissions: [Agent Skills](https://platform.claude.com/docs/en/managed-agents/skills),
  [Authenticate with vaults](https://platform.claude.com/docs/en/managed-agents/vaults),
  and [Permission policies](https://platform.claude.com/docs/en/managed-agents/permission-policies).
- Stable API snapshot: `anthropics/skills` commit
  [`9d2f1ae`](https://github.com/anthropics/skills/commit/9d2f1ae187231d8199c64b5b762e1bdf2244733d),
  especially [endpoint and lifecycle semantics](https://github.com/anthropics/skills/blob/9d2f1ae187231d8199c64b5b762e1bdf2244733d/skills/claude-api/shared/managed-agents-api-reference.md#L1-L60),
  [session and agent fields](https://github.com/anthropics/skills/blob/9d2f1ae187231d8199c64b5b762e1bdf2244733d/skills/claude-api/shared/managed-agents-core.md#L79-L158),
  and [multi-agent roster, thread, and limit semantics](https://github.com/anthropics/skills/blob/9d2f1ae187231d8199c64b5b762e1bdf2244733d/skills/claude-api/shared/managed-agents-multiagent.md#L1-L56).

## The `agent` noun (primary-source quotes)

- "An agent is a **reusable, versioned configuration** that defines persona
  and capabilities. It bundles the model, system prompt, tools, MCP servers,
  and skills that shape how Claude behaves during a session."
  ([Agent setup](https://platform.claude.com/docs/en/managed-agents/agent-setup))
- Concept table: "**Agent** | The model, system prompt, tools, MCP servers,
  and skills" and "**Session** | A running agent instance within an
  environment, performing a specific task."
  ([Overview](https://platform.claude.com/docs/en/managed-agents/overview))
- "Create the agent once as a reusable resource and reference it by ID each
  time you start a session. Agents are versioned and easier to manage across
  many sessions."
  ([Agent setup](https://platform.claude.com/docs/en/managed-agents/agent-setup))
- "Step one of every flow. Sessions require a pre-created agent, there is no
  inline agent config under `managed-agents-2026-04-01`."
  ([API reference snapshot](https://github.com/anthropics/skills/blob/9d2f1ae187231d8199c64b5b762e1bdf2244733d/skills/claude-api/shared/managed-agents-api-reference.md#L49-L60))
- The API object is a persistent, versioned resource: `id`
  (`agent_01...`), `type: "agent"`, `name`, `model` (`{id, speed}`), `system`,
  `tools`, `skills`, `mcp_servers`, `metadata`, `version`, `created_at`,
  `updated_at`, `archived_at`.
- **Conceptual model: agent-as-config with strong identity.** A server-side,
  versioned configuration resource; the running thing is always the session.

## Subagents

- First-class, via the `multiagent` field: "Multi-agent orchestration lets one
  agent coordinate with others... Agents can act in parallel with their own
  isolated context."
  ([Multi-agent sessions](https://platform.claude.com/docs/en/managed-agents/multi-agent))
- **Declared, not ad-hoc:** "When defining your agent, set `multiagent` to
  declare the roster of agents the coordinator can delegate to." Roster
  entries are other agents by id (optionally pinned to a `version`) or
  `{"type": "self"}` (the coordinator spawns copies of itself). The roster is
  "snapshotted when the coordinator is created or updated", referenced agents
  "do not automatically pick up later updates to their definitions."
- **Spawned at runtime as session threads:** "each agent runs in its own
  **session thread**, a context-isolated event stream with its own
  conversation history"; "additional threads are spawned at runtime when the
  coordinator delegates work."
- **Inherited vs isolated:** "All agents share the same sandbox, filesystem,
  and vault credentials"; "Each agent uses its own configuration: model,
  system prompt, tools, MCP servers, and skills... Tools, MCP servers, and
  context are not shared." Session-level overrides apply only to the
  coordinator and its `self` copies.
- **Limits:** "The coordinator can only delegate to one level of agents;
  depth > 1 is ignored. A maximum of 20 unique agents can be listed in
  `multiagent.agents`, but the coordinator can call multiple copies of each
  agent." "A maximum of 25 concurrent threads are supported."
- **Communication:** message passing between threads, visible as events
  (results delivered with `from_session_thread_id`/`from_agent_name`;
  follow-ups with `to_session_thread_id`). "Threads are persistent: the
  coordinator can send a follow-up to an agent it called earlier, and that
  agent retains everything from its previous turns."

## Configuration surface (what, where, why)

Agent-level fields (agent-setup): `name` (required), `model` (required, id or
`{id, speed}`), `system` (up to 100k chars), `tools` (pre-built
`agent_toolset_20260401`, MCP, custom), `mcp_servers`, `skills` (max 20 per
session), `multiagent` (coordinator roster), `description`, `metadata` (max
16 pairs).

Session-level: `agent` (id, pinned version, or overrides object),
`environment_id`, `vault_ids` (session-scoped credentials), `resources`
(memory stores, files, GitHub repos), `title`, `metadata`.

Environment-level (a separate resource): `type` (`cloud` | `self_hosted`),
`packages` (apt/cargo/gem/go/npm/pip), `networking` (`unrestricted` or
`limited` with `allowed_hosts`).

Where config lives: server-side API objects (Console dashboard on top);
skills are filesystem bundles (`SKILL.md` + files) uploaded via the Skills
API. Notably there is no repo-file config, no AGENTS.md/CLAUDE.md; that is a
Claude Code concept, and branding rules explicitly separate the products
("Not permitted: 'Claude Code' or 'Claude Code Agent'").

Stated rationale for knobs:

- Skills: "Unlike prompts (conversation-level instructions for one-off
  tasks), skills load on demand, only impacting the context window when
  needed." (managed-agents/skills)
- Environments: "You create an environment once, then reference its ID each
  time you start a session." (managed-agents/environments)
- Vaults: "The vault reference is a per-session parameter, so you can manage
  your product at the `agent` resource granularity and your users at the
  `session` resource granularity." (managed-agents/vaults), i.e. agent =
  product config, session = end-user instance.
- Permission policies: MCP tools default to `always_ask` because "new tools
  added to an MCP server do not execute in your application without
  approval." (managed-agents/permission-policies)

## Binding time

- **Agent create/update:** `model`, `system`, `tools`, `mcp_servers`,
  `skills`, `multiagent` are bound to a numbered version. "Updating an agent
  generates a new version when the configuration changes. The `version` field
  is required and must match the agent's current version... A version
  mismatch returns a 409." No-op updates create no version. The multiagent
  roster snapshots referenced agents' versions at that moment.
- **Session create:** the session binds an agent (latest or pinned version)
  plus per-session overrides, "Overrides apply only to the session you
  create. They do not modify the agent resource or create a new agent
  version." `vault_ids` and memory stores bind here; "Memory stores can only
  be attached at session creation time."
- **Mid-session (partially mutable):** "You can update a session's
  `agent.tools` and `agent.mcp_servers`, including permission policies,
  mid-session... Updates are session-local and do not propagate back to the
  underlying agent." "Only the agent's `tools` and `mcp_servers` can change
  after a session is created," and "The session must be `idle` to update the
  agent." The `system` field is fixed for the session's lifetime (on Opus 4.8
  a `system.message` event can replace the effective prompt between turns).
  Files and GitHub repos can be added post-creation via the resources
  endpoint.
- **Archive:** "Archiving makes the agent read-only and cannot be undone.
  Existing sessions continue to run, but new sessions cannot reference the
  agent."

## Relationships between nouns

- **Agent : Session, 1:many.** Session = "a running agent instance within an
  environment." Sessions cannot exist without a pre-created agent.
- **Agent : Environment, orthogonal.** Independent resources, both
  referenced at session creation. Environment = where; agent = who/what.
- **Environment : Session, 1:many; Session : Sandbox, 1:1.** "Multiple
  sessions can share the same environment, but each session gets its own
  isolated sandbox (a fresh Linux container)."
- **Session : Thread, 1:many** (multi-agent): the coordinator runs in the
  primary thread; each delegation spawns a thread (`sth_…`), max 25
  concurrent.
- **Session : Events, 1:many.** Events (`sevt_…`) are the sole interaction
  channel, persisted server-side, streamable over SSE.
- **Deployment : Session, 1:many.** A deployment (scheduled trigger,
  `depl_…`, states active | paused | archived) creates a new session per
  firing, with run records (`drun_…`).
- **Vault : Credential, 1:many** (max 20); vaults attach per-session and
  apply to every thread.
- **MemoryStore : Session, many:many** (max 8 per session); a store holds up
  to 2,000 path-addressed memories, each with immutable versions.
- **Self-hosted sandboxes:** "Self-hosted sandboxes keep the orchestration on
  Anthropic's side but move tool execution into infrastructure you control",
  the environment acts as a work queue the customer's worker polls
  (`/v1/environments/{id}/work`). Providers include AWS Lambda MicroVMs,
  Blaxel, Cloudflare, Daytona, E2B, GKE Agent Sandbox, Modal, Namespace,
  Superserve, Vercel.
- Every noun has an ID prefix (agent_, session_, env_, vlt_, memstore_,
  mem_, depl_, sth_, sevt_, outc_...), underscoring that these are all
  persistent API resources.

## Lifecycle

- **Agent:** create (`POST /v1/agents`, version 1) → update (optimistic
  concurrency on `version`, increments) → archive (read-only, permanent). No
  hard delete documented. The agent has no runtime states, it is a resource,
  not a process.
- **Session:** statuses `idle` (starts here), `running`, `rescheduling`
  (transient error, auto-retry), `terminated` (unrecoverable). "Sessions
  follow a two-step lifecycle: first create the session to provision its
  sandbox, then send a user event to start work." Archive preserves history
  (running sessions must be interrupted first); delete permanently removes
  "its record, events, and associated sandbox."
- **Who owns the loop: Anthropic.** "Instead of building your own agent loop,
  tool execution, and runtime, you get a fully managed environment where
  Claude can read files, run commands, browse the web, and execute code
  securely." (overview) Even self-hosted sandboxes only move tool execution;
  orchestration stays on Anthropic's control plane.
- **Persistence:** conversation/event history and the session filesystem
  persist for the session's life ("Stateful sessions: Persistent filesystems
  and conversation history across multiple interactions"). Across sessions,
  persistence is opt-in via memory stores: "Memory stores let the agent carry
  information across sessions: user preferences, project conventions, prior
  mistakes, and domain context." A "Dreams" research preview consolidates
  memory.
- **Outcomes:** a `user.define_outcome` event turns a session into
  rubric-evaluated work with a separate grader context and bounded iterations
  (default 3, max 20), the platform's notion of "done."

## What makes it "an agent" here (our inference)

Our inference: in Claude Managed Agents, an agent is a **server-side,
versioned identity-plus-configuration resource**, persona (system prompt),
brain (model), and capabilities (tools, MCP servers, skills, delegation
roster), that the platform instantiates as sessions inside sandboxed
environments. What separates it from a plain LLM call is that Anthropic runs
the entire loop (tool execution, threads, retries, memory, scheduling)
against that resource; what separates it from OpenComputer-style "agent =
config" is that the configuration is richer (permissions, credential vaults,
declared multi-agent rosters) and the version is a first-class,
optimistically-locked field on the resource itself.

## Open questions

- No hard delete for agents (only archive), is deletion possible at all?
- The events-and-streaming reference was only partially readable; the full
  event taxonomy (especially thread-level events) is not captured here.
- "Dreams" (memory consolidation) is a research preview with thin docs.
- How session-level `agent.tools` updates interact with a multiagent roster
  (do subagent threads see coordinator tool updates?) is not documented.
