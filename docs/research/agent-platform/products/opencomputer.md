# OpenComputer: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence from the official docs plus the code in `diggerhq/opencomputer`
(TypeScript SDK, web Zod schemas, design docs) and `diggerhq/oc-runtimes`.

## The `agent` noun (primary-source quotes)

- Docs definition: "An **agent** is reusable configuration: `name`, `prompt`,
  `model`, `runtime`, and optional **skills**."
  (docs.opencomputer.dev/agent-sessions/agents)
- "Every session runs on an agent, so create one first, then start as many
  sessions on it as you like." (agent-sessions/agents)
- "The only thing an agent needs is a **model** and a **prompt**, create one
  and it runs on a default, managed runtime that just works."
  (agent-sessions/agents)
- SDK class comment: `/** Reusable agents, the "what" a session runs. */`
  (`sdks/typescript/src/agents/agents.ts:60`)
- The SDK type is a persistent record, not a process
  (`sdks/typescript/src/agents/types.ts:32-45`):

```typescript
export interface Agent {
  id: string;
  name: string;
  promptHash?: string;
  model: string;
  runtime: Runtime;
  revision?: number;
  /** The agent's active revision (what new sessions run); `prompt`/`model` are served from it. */
  activeRevision?: { id: string; number: number; digest: string } | null;
  credentialId?: CredentialRef | null;
  limits?: Limits;
  createdAt?: string;
  updatedAt?: string;
}
```

- Creation is idempotent by name: "Idempotent by name server-side → safe to
  auto-retry transient failures." (`agents.ts:85`)
- **Conceptual model: agent-as-config.** The agent is a named, versioned,
  persistent configuration record. Execution lives entirely on the session;
  the agent object holds no runtime state.
- Caveat: the codebase carries a second, older "agent" concept, the
  sandbox-embedded agent in the Python SDK (`sdks/python/opencomputer/agent.py`,
  `POST /sandboxes/:id/agent`), which is a Claude Code process running inside
  a sandbox VM. The primary current product (v3 Durable Agent Sessions) is the
  agent-as-config model above. A separate older repo `diggerhq/oc-agents` uses
  "agent" to mean roughly what v3 calls a session.

## Subagents

- Not in the platform data model. No `parent_id`, `subagent`, or child-agent
  concept exists in the SDK types, Zod schemas, or design docs; built-in
  runtimes (`claude`, `codex`, `pi`) expose no subagent-spawning tool.
- Subagents exist only inside the **Flue** framework runtime, in-process and
  explicitly second-class: "Subagents (`session.task()`) bundle and execute
  in-process, but they are **outside the tested profile**: their model
  declarations aren't validated the way the top-level agent's is, and subagent
  steps may appear incompletely in the session event log."
  (agent-sessions/flue)
- The platform's sanctioned fan-out unit is the session, not the subagent:
  "One agent, one conversation per session... **fan out by creating
  sessions**." (agent-sessions/flue)
- Sessions do not share memory, so fanned-out sessions are isolated: "**No
  cross-session memory.** Each session starts from the agent's prompt + that
  session's `input`." (agent-sessions/sessions)
- Events can be attributed to an actor of `type: "human" | "agent" | "system"`
  (`types.ts:26-30`), but this is attribution, not hierarchy.
- Nesting limits, inheritance rules, and parent-child communication are
  undocumented (the concept barely exists).

## Configuration surface (what, where, why)

Agent-level fields (`CreateAgentParams`, `agents.ts:9-21`):

- `name`, identity; creation is idempotent by name.
- `prompt`, required instructional text.
- `model`, required `provider/model` id; "the provider must be one the
  runtime can drive (`anthropic/…` for `claude`, `openai/…` for `codex`, any
  catalog provider for `pi`)."
- `runtime`, `claude` | `codex` | `pi` | `flue` | custom; "immutable after
  create."
- `key` / `credential`, inline provider key (sealed into a credential) or
  `cred_…` id or the `"managed"` sentinel ("run via OpenComputer, no provider
  key"); omit to inherit the org default.
- `limits`, `{ tokens?, turnSeconds?, turns? }`.
- `skills`, folder-based instruction sets (`SKILL.md` with YAML frontmatter),
  deployed as part of revisions, not a create param.

Per-session overrides (`CreateSessionParams`): `model`, `revision` (pin a
specific revision), `limits`, `sources` (repos checked out into
`/workspace/sources/`), `metadata` (opaque JSON ≤16 KB, not shown to the
model), `key` ("get-or-create idempotency/routing key, one session per
key"), `input`, webhook `destinations`.

Where config lives:

- API objects (`POST /agents`, SDK `oc.agents.create({})`).
- A repo file layout for push-to-deploy: `agent.toml` (`[agent].id`,
  `model = "..."`) + `prompt.md` + optional `skills/` directory.
- Credentials in a secrets vault (Infisical), referenced by `cred_…` id.
- At runtime, config reaches the sandbox as `OC_*` env vars
  (`OC_AGENT_PROMPT`, `OC_MODEL`, `OC_SKILL_BUNDLE_DIGEST`, ...)
  (`oc-runtimes/README.md`).

Stated rationale for the knobs:

- Revisions: "every deployment... appends a new one, and **you can roll back
  instantly**." (agent-sessions/revisions)
- Skills: give the agent "a playbook it reaches for, **without bloating every
  prompt**", progressive disclosure to keep context lean.
  (agent-sessions/skills)
- Brain/hands sandbox split: "Commands and file edits stay isolated from the
  model loop." (agent-sessions/runtimes)
- Sealed credentials: "The real key never enters the sandbox." / "once
  submitted it's never returned by the API." (agent-sessions/credentials)
- Managed credential: run "billed to your credits, with no key to manage."
  (agent-sessions/credentials)

## Binding time

- **Agent create (immutable):** `runtime`, "to switch engines, create a new
  agent." (agent-sessions/runtimes)
- **Agent update / deploy (versioned):** `prompt`, `model`, `skills` are
  served from the active revision; every deploy appends an immutable, linear,
  numbered revision ("like a Vercel/Fly deployment... never rewritten").
  Rollback = activate an earlier revision. `activate: false` stages a revision
  without making it live.
- **Session create (frozen):** "When a session is created it **freezes** the
  agent's active revision, `prompt`, `model`, `runtime`, and `skills`, into
  the session, and pins the resolved credential too." (agent-sessions/agents)
  The pinned copy lives on the session as `agentSnapshot`
  (`types.ts:73`).
- **Mid-session (pinned, one exception):** "Editing the named agent afterwards
  never changes a running or resumed session, its behavior is pinned for its
  whole life." (agent-sessions/agents) The exception is credential rotation,
  "the one exception to a session's otherwise pinned config"
  (agent-sessions/credentials). Steering (new messages) and webhook
  destinations are mutable mid-session; behavior config is not.
- **In-flight vs new:** "In-flight sessions keep the revision they started on;
  only new sessions pick up a change." (agent-sessions/revisions)

## Relationships between nouns

- **Agent → Session: 1:many.** Session has `agentId`; sessions list filters by
  agent. Session is the run; agent is the template.
- **Agent → Revision: 1:many, linear.** Exactly one active revision at a time;
  the agent holds an `activeRevision` pointer.
- **Agent → Deployment: 1:many.** A deployment (states: accepted → queued →
  ... → ready | failed | superseded) produces a revision.
- **Agent → Schedule: 1:many.** A schedule (`sch_…`, states active | paused |
  auto_paused) fires one new session per cron slot.
- **Session → Sandboxes: one brain + one hands.** "The runtime runs the agent
  loop in its **own per-session sandbox** (the *brain*)... file and shell
  tools run in a **separate sandbox** (the *hands*)."
  (agent-sessions/runtimes; `SessionSchema.sandboxes.{brain,hands}` in
  `web/src/api/schemas.ts:503-507`). Sandboxes are owned by the end-user org
  (design doc `agent-sandbox-ownership.md`).
- **Session → Turn: 1:many.** A turn is one wakeup cycle of the loop; each
  steer message starts a new turn.
- **Session → event log: 1:1, append-only.** "A **session** is an agent run
  with an **append-only event log**, a pinned agent snapshot, and a lifecycle
  status." (agent-sessions/sessions)
- **Session → artifacts, watches, channels, destinations: 1:many.** Watches
  wake the session on third-party GitHub PR changes ("A session can only watch
  a PR it opened itself"); channels bind a session to an external thread
  (Slack/GitHub, Labs preview).
- **Org** is the ownership boundary for agents, sessions, sandboxes, and
  credentials.
- **Task** is not a platform noun (only Flue's in-process `session.task()`).
  **Workflow** is not an API noun. **Workspace** is just the
  `/workspace/sources/` directory convention inside the hands sandbox.
  **Tools** are functions exposed to the runtime over MCP during active turns
  (`bash`, `read`, `write`, `say`, `ask`, `github_publish_pull_request`, ...).

## Lifecycle

- **Agent:** no state machine, it is a definition, not a process. Create
  (`POST /agents`, idempotent by name), update (`PATCH`), deploy revisions,
  activate/rollback revisions. No delete endpoint found in the SDK.
- **Session:** explicit status enum (`types.ts:4`): `queued`, `running`,
  `awaiting_input`, `idle`, `failed`, `archived`; yield reasons `completed |
  needs_input | error | budget_exceeded | deadline_exceeded | max_turns |
  canceled`.
- **Hibernate/resume:** "With nothing left to do, the session goes **idle**
  and the sandbox **hibernates**." A steer message "wakes it; it resumes with
  its prior context." (agent-sessions/sessions, messaging)
- **Stop:** `cancel` is a cooperative stop; "**`archive` is not delete**, the
  session's log is retained, just read-only." (agent-sessions/sessions)
- **Who owns the loop: the platform.** "A **runtime** executes the agent loop:
  provider SDK, model calls, and tool use." (agent-sessions/runtimes) The host
  execs the runtime adapter once per turn; no customer process is needed
  between turns. Custom runtimes invert this partially, "run your own agent
  harness on Durable Agent Sessions" by implementing a `POST /turn` HTTP
  handler the adapter calls, but scheduling, durability, and the event log
  stay platform-owned.
- **Persistence:** the brain sandbox is resident, "Survives across turns and
  hibernate/wake (same pid), so SDK boot is paid once per box, not per turn"
  (`oc-runtimes/README.md`); the event log is permanent; runtime state
  checkpoints under a state dir ("Work since the last logged event may
  repeat" on crash recovery). Across sessions, nothing persists: no
  cross-session memory by design.

## What makes it "an agent" here (our inference)

Our inference: in OpenComputer, "agent" is a **named, versioned configuration
record**, prompt + model + runtime + credential + limits + skills, that the
platform can instantiate into any number of durable sessions. The agentic
behavior (the loop, tools, sandboxes, persistence) belongs to the
session/runtime pair, not to the agent object; what separates an agent from a
plain LLM call is that the platform runs a managed, tool-using, resumable loop
against that configuration, with the session as the unit of execution and the
agent as the unit of identity, versioning, and reuse.

## Open questions

- Agent deletion: no delete endpoint found in docs or the TypeScript SDK; is
  the agent lifecycle create/update-only?
- Flue subagent semantics (inheritance, limits, event-log fidelity) are
  explicitly "outside the tested profile", undefined rather than documented.
- Whether the sandbox-embedded Python-SDK agent (`/sandboxes/:id/agent`) is
  deprecated in favor of v3 Durable Agent Sessions, or a supported parallel
  API.
- Cardinality of Slack channel binding is stated as "1 app ⟷ 1 agent ⟷ 1
  workspace" in code comments while sessions support multiple channel
  bindings; the exact constraint model is unclear (Labs preview).
