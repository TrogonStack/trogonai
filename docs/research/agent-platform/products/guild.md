# Guild: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence from unversioned live documentation at docs.guild.ai, fetched as raw
markdown by appending `.md` to each doc path (for example
`https://docs.guild.ai/platform/agents.md`), enumerated from the `llms.txt`
page index, plus the OpenAPI document embedded in every `api-reference` page
and served at `https://docs.guild.ai/api-reference/openapi.json`. Component
versions are the ones the docs declare about themselves. Retrieved 2026-09-17.

## Source anchors

The docs are unversioned live pages. There is no changelog, no release-notes
page, and no doc version stamp anywhere in the full page index
(`https://docs.guild.ai/llms.txt`), and the site publishes no archive or
snapshot URL and no content digest. Retrieval date plus the component versions
the docs declare about themselves is therefore the strongest available pin, and
the records below rest on local raw-markdown copies taken on the retrieval
date.

- `@guildai/agents-sdk` 0.6.0 with Zod `~4.3.0`
  (https://docs.guild.ai/packages/agents-sdk, "Version and Zod requirement").
- The public API's OpenAPI `info.version: 1.0.0`, served at
  `https://api.guild.ai/v1` (embedded OpenAPI in every
  `https://docs.guild.ai/api-reference/...` page).
- The pinned Codex CLI `@openai/codex@0.146.0`
  (https://docs.guild.ai/guide/codex-driver, "Execution environment").
- Deprecation markers give one more internal ordering signal: the SDK
  `description` field is "(optional, deprecated as of `@guildai/agents-sdk`
  0.4.0)" (https://docs.guild.ai/guide/sdk-introduction, "Agent schema").

Two mechanical adaptations apply to quoted passages, so the quotes stay
verbatim in text while this page still renders. Relative links inside a quote
are rewritten to their absolute `https://docs.guild.ai` form, since a
site-relative target would resolve against this site. Template placeholders
that Guild writes bare, such as the workspace-variable reference, are marked as
code so the renderer does not evaluate them; where a placeholder cannot survive
that treatment the quote elides it and says so.

Primary pages behind this dossier: `index`, `quickstart`,
`platform/{agents,sessions,workspaces,publish-to-agent-hub,goose-recipes,skills,context,workspace-variables,environments,credentials,credential-policies,llm-settings,integrations,triggers,event-triggers,schedule-triggers,api-triggers,security-architecture,organizations,artifacts,evaluations}`,
`guide/{agent-types,sdk-introduction,native-agents,llm-agents,coded-agents,self-managed-agents,goose-agents,openclaw-agents,versions,tasks,state,llms,codex-driver}`,
`sdk/{task-object,tools,get-self,mcp-integrations}`,
`packages/{agents-sdk,babel-plugin}`, `cli/{getting-started,commands,skills,mcp-server}`,
`reference/{limits,evals}`, `insights/audit-logs`, and the `api-reference`
tree.

## The `agent` noun (primary-source quotes)

- **An agent is a program with a typed input and a typed output, stated three
  times in compatible wording.** "Agents are programs that accept input, use
  tools and LLMs, and produce output. They range from simple scripted programs
  to autonomous LLM-driven workflows." (https://docs.guild.ai/index, "Core
  concepts", Agents card). The platform page varies only the tail: "They range
  from simple prompt-driven assistants to deterministic TypeScript workflows."
  (https://docs.guild.ai/platform/agents, lead paragraph). The SDK page is the
  tightest: "An agent takes typed input, does its work using tools, LLMs, or
  other agents, and returns typed output."
  (https://docs.guild.ai/guide/sdk-introduction, lead paragraph). Note that
  delegation is in the definition itself: "or other agents".
- **Product framing is control-plane framing.** "The control plane for AI
  agents." and "Guild is a platform for building, deploying, and governing AI
  agents." (https://docs.guild.ai/index, "Introduction"). Determinism is the
  repeated selling axis, and it is backed by a named property rather than
  asserted loosely: the agent-type comparison table's own column headers are
  "Control" with values "Stochastic" or "Deterministic" and "LLM cost" with
  values "Variable" or "Fixed"
  (https://docs.guild.ai/guide/sdk-introduction, "Choosing an agent type").
- **The agent is a durable platform record, not a process.** From
  `components.schemas.Agent` in the embedded OpenAPI
  (https://docs.guild.ai/api-reference/agents/create-an-agent): `id`
  (`format: uuid`), `created_at`, `updated_at`, `owner_id` ("The account that
  owns this entity."), `name` ("Unique name identifying the asset within the
  owner account.", `maxLength: 100`), `full_name` ("Full agent name in format"
  followed by the literal `owner_name/agent_name`), `git_url` ("URL to the
  agent's git repository"), `agent_type`, `is_public` ("Whether the asset is
  visible to the community or only its owner."), `archived_at`,
  `archived_by_id`, `maintainer_id`, `category_id`, `forked_from_id`,
  `installs_count`, `forks_count`, `likes_count`, `moderation_state`, and
  `generated_description`. The record carries its own provisioning status:
  `status` with values `CREATED`, `GIT_REPOSITORY_CREATED`, `READY`.
- **Every agent is git-backed, and a commit has a record-level effect.**
  `generated_description` is "LLM-generated description based on agent code.
  Regenerated automatically on every commit." (same schema). `forked_from_id`
  is "If applicable, the ID of the agent version this agent was forked from.",
  so forking copies from a *version* into a new *agent*.
- **The record can exist before any code does.** `POST /agents` requires only
  `name`; `agent_type` is not in the request body and `template` is an
  optional `AgentCreationTemplate` (`CreateAgentInput`, `required: [name]`).
  The CLI does both acts at once and says so: "This scaffolds a
  `hello-agent/` directory, creates the agent in the Guild backend,
  initializes a local git repo, and pulls starter files:"
  (https://docs.guild.ai/quickstart, "Create, test, publish").
- **The run-time nouns are separate and explicitly logs rather than agents.**
  "A session is a log of an agent's run. Input or a trigger is provided, the
  agent runs, asks questions if needed, and returns output or achieves an
  outcome." (https://docs.guild.ai/index, "Core concepts", Sessions card);
  "A session is a conversation with an agent."
  (https://docs.guild.ai/platform/sessions, lead paragraph). The runtime
  "loads the agent code, executes it, and manages the agent lifecycle."
  (https://docs.guild.ai/index, "How agents work", step 3). And "Every agent
  receives a `Task` object as its second argument. The task is your agent's
  interface to the Guild runtime" (https://docs.guild.ai/guide/tasks, lead
  paragraph).

### Agent types: the taxonomy does not agree with itself

The docs count the agent types three different ways, and the discrepancy is
worth recording rather than smoothing over.

- Prose says four: "Guild supports four agent execution architectures. Your
  choice determines the programming model, the files scaffolded at
  initialization, and how the agent transitions to `READY`."
  (https://docs.guild.ai/guide/agent-types, lead paragraph), repeated as "See
  [Agent types](https://docs.guild.ai/guide/agent-types) for a full overview of the four execution
  architectures." (https://docs.guild.ai/guide/sdk-introduction).
- Six authoring choices are presented as peer cards and as one comparison
  table: Native, TypeScript LLM, Auto-managed state, Self-managed state,
  Goose, OpenClaw (https://docs.guild.ai/guide/sdk-introduction, "Agent types"
  and "Choosing an agent type"; https://docs.guild.ai/platform/agents, "Build
  your own").
- The record has five values, and the creation template has seven.
  `agent_type` is `[GUILD_TYPESCRIPT, GUILD_NATIVE, GOOSE, OPENCLAW,
  LANGGRAPH]` and `AgentCreationTemplate` is `[LLM, AUTO_MANAGED_STATE, BLANK,
  GOOSE, GUILD_NATIVE, OPENCLAW, LANGGRAPH]`
  (https://docs.guild.ai/api-reference/agents/create-an-agent).

The reconciliation for four versus six is that the three TypeScript flavors
share one architecture: "Every agent has a mandatory `agent_type` property
that identifies how it is implemented. The platform sets this value
automatically and exposes it in the API.", and `'GUILD_TYPESCRIPT'` is "A
TypeScript agent, whether built with `llmAgent` or with the `agent` function
(auto-managed or self-managed state)."
(https://docs.guild.ai/guide/sdk-introduction, "Agent type identifiers"). The
reconciliation for `LANGGRAPH` is that there is none: no page in the index
documents it. That gap is recorded below.

The comparison table's own axes, quoted cell by cell from
https://docs.guild.ai/guide/sdk-introduction ("Choosing an agent type"):

| Agent type | Written in | Ease | Control | LLM cost |
| --- | --- | --- | --- | --- |
| Native | "Markdown prompt" | "Easiest" | "Stochastic" | "Variable" |
| TypeScript LLM | "TypeScript" | "Easy" | "Stochastic" | "Variable" |
| Auto-managed state | "TypeScript" | "Moderate" | "Deterministic" | "Fixed" |
| Self-managed state | "TypeScript" | "Challenging" | "Deterministic" | "Fixed" |
| Goose | "YAML recipe" | "Easy" | "Stochastic" | "Variable" |
| OpenClaw | "Markdown workspace" | "Easy" | "Stochastic" | "Variable" |

What distinguishes each, in the docs' own words:

- **Native (`GUILD_NATIVE`).** "A Native agent is a prompt and a tool list."
  and "`PROMPT.md` is the whole agent. Its contents become the system prompt,
  and it is the only required file"
  (https://docs.guild.ai/guide/native-agents). "Guild runs the agentic loop
  itself, so there is no container and no filesystem. Guild Native agents take
  text in and return text out." and "Guild Native agents skip the build
  validation step. After initialization, the agent immediately transitions to
  `READY`." (https://docs.guild.ai/guide/agent-types). Also "Native agents are
  deliberately narrow." (https://docs.guild.ai/guide/native-agents, "What
  Native agents do not do").
- **TypeScript (`GUILD_TYPESCRIPT`).** "TypeScript agents use the
  `@guildai/agents-sdk` to define behavior in code. After you save a version,
  the agent goes through a build and validation step before it transitions to
  `READY`." (https://docs.guild.ai/guide/agent-types). Its three templates
  differ by who drives control flow: `llmAgent` "pairs a system prompt with a
  tool set" (https://docs.guild.ai/guide/llm-agents); auto-managed state
  agents "are TypeScript functions you write yourself. They execute
  deterministically from start to finish, with no LLM driving the control
  flow" (https://docs.guild.ai/guide/coded-agents); and
  "`SelfManagedStateAgent` is an event-driven state machine. Instead of a
  single `run` function, you implement two callbacks"
  (https://docs.guild.ai/guide/self-managed-agents).
- **Goose (`GOOSE`).** "Guild validates the recipe at build time and runs it as
  a headless agent." and "The agent version's files must include a
  `recipe.yaml` at the root."
  (https://docs.guild.ai/guide/goose-agents;
  https://docs.guild.ai/platform/goose-recipes). Platform control is enforced
  by rejecting recipe fields: `settings` is "**Rejected**" because "provider
  and model are platform-controlled.", `extensions` is "**Rejected**" because
  "Guild does not honor recipe extensions.", `sub_recipes` and `retry` are
  "**Rejected**", and "Any unknown field" is "**Rejected**" with the reason
  "catches typos and unreviewed future fields."
  (https://docs.guild.ai/guide/goose-agents, "Recipe fields").
- **OpenClaw (`OPENCLAW`).** "An OpenClaw agent is a workspace directory." and
  "`AGENTS.md` *is* the agent: what you write there is what the model is told
  about who it is and how to behave."
  (https://docs.guild.ai/guide/openclaw-agents). The toolchain is not declared
  but assumed: "is built in. A shell, file editing, `git`, and search are
  always available and need no declaration." (same page, "Tools"; the bolded
  subject in the source is "The coding toolchain"). No environment pinning: "An
  OpenClaw agent always runs in the standard `guildai~lobsterpot` image."

The stated axis between Native and the container-backed types is startup cost:
"Because the loop runs inside Guild, a Native agent also starts faster than
the container-backed types. Goose and OpenClaw agents each get their own
container, created when the task starts and destroyed when it ends; a Native
agent has nothing to start, so its first turn begins as soon as the task is
dispatched." (https://docs.guild.ai/guide/native-agents).

### The record versus the workspace installation of it

These are two resources, and the distinction is load-bearing. Installing
produces a `WorkspaceAgent` row with its own uuid, reachable at
`/workspace_agents/{workspace_agent_id}`, created by
`POST /workspaces/{workspace_id_or_name}/workspace_agents`. Its fields,
verbatim from `components.schemas.WorkspaceAgent.properties`
(https://docs.guild.ai/api-reference/workspaces/add-an-agent-to-a-workspace):
`creator_id` ("The user or API key that added this agent version to the
workspace."), `version_id` (a **version**, not an agent), `workspace_id`,
`is_default_chat_agent` ("If True, this workspace agent is used for new chat
sessions when no explicit agent is provided."), `should_autoupdate` ("If True,
the agent will automatically update when a new version is published."), and
`required_credentials_mode` ("Whose credentials this agent may run with:
SHARED = only org-side credentials serve, MEMBER = only the acting member's
own.").

So an install pins one version id and carries its own credential posture,
where the workspace's own declaration is the fallback: "A workspace agent's own
declaration overrides this one."
(`components.schemas.Workspace.properties.required_credentials_mode`). Three
further signals that record and install are distinct: archiving the record
reaches through every install ("Archiving an agent removes it from every
workspace it is installed in and stops it from running.",
https://docs.guild.ai/platform/agents); budgets exist at both levels ("An agent
can carry its own monthly spend budget", same page, "Spend budget"); and the
record and the agent's self-description may legitimately diverge ("Committing
an `IDENTITY.md` that differs from the agent's Guild record is not an error.
The record drives the Agent Hub listing and the session UI; the file drives
what the model believes about itself.",
https://docs.guild.ai/guide/openclaw-agents).

### Versions and publishing to Agent Hub

"Every time you save an agent, Guild creates a new version. Versions let you
iterate on agent code while keeping previous versions available."
(https://docs.guild.ai/guide/versions). Three version states, verbatim:
**Saved** "Code is uploaded and stored"; **Validated** "Runtime has verified
the agent builds and conforms to its schemas"; **Published** "Available to
your organization for installation" (same page, "Version states").

`AgentVersion` is `oneOf` `AgentVersionCommitted` or `AgentVersionEphemeral`,
with `version_type` in `[EPHEMERAL, COMMITTED]`
(https://docs.guild.ai/api-reference/agents/list-an-agents-versions). The
committed shape is where the build's conclusions are recorded: `version_number`
("The semantic version number (i.e., MAJOR.MINOR.PATCH) of this version"),
`sha` ("Git commit SHA (40-character hexadecimal). For agents: set on version
creation."), `validation_status` (`SKIPPED`, `PENDING`, `RUNNING`, `PASSED`,
`FAILED`), `input_schema` ("JSON Schema from Zod input"), `output_schema`
("JSON Schema from Zod output"), `env_references` ("Workspace variable keys
referenced as <code v-pre>{{env.KEY}}</code> in the version's prompt, extracted at build time"),
`raw_tools` ("Structured per-tool metadata. Each entry is a tool object
discriminated by toolType (integration, legacy, agent, builtin)."),
`dependencies` ("npm package.json dependencies"), and
`runtime_environment_id`. The ephemeral shape instead carries code inline:
`files` ("The files for this version and their content. Format: {[key:
filename]: content}").

Publishing is a version-level act with agent-level visibility consequences.
"Publishing makes an agent version available for installation."
(https://docs.guild.ai/platform/publish-to-agent-hub), via
`POST /versions/{version_id}/publish`. Three visibility states, verbatim:
"**Draft**: no published version. The agent is not available for
installation."; "**Internal**: published with visibility restricted to your
organization (`is_public=false`). Not listed on Agent Hub, but installable by
workspaces in your account"; "**Public**: published and listed on Agent Hub
(`is_public=true`). Installable by anyone." Internal is the default, and public
is a one-way door: "Making an agent public is permanent. Once an agent is
public (`is_public=true`), you cannot revert it to a private or
organization-only state."

Unpublish exists but refuses to break installs: "Unpublishing moves the
version from `PUBLISHED` back to `DRAFT`" and "A version that is still in use
cannot be unpublished. If any workspace has that version installed, the
request is refused and the version stays published, so unpublishing never
breaks an existing install." (same page, "Unpublish").

**Conceptual model: agent-as-config-record plus immutable versioned code.**
Owned by an account, addressable by UUID or `owner/name`, git-backed, with
sessions and tasks as the ephemeral run nouns. Nothing in these pages models
the agent as a long-lived process, and nothing gives it a persistent
filesystem or memory store of its own.

## Subagents

Yes, and the naming is inconsistent across surfaces in a way worth mapping:
**sub-agents** (hyphenated) in prose and configuration, `sub_agents` in YAML,
`toolType: "agent"` in the version's tool manifest, `guildAgentTool` as the
SDK constructor, `EntTaskAgent` / `TaskAgent` in the API, and **sub-tasks**
when describing the runtime records they produce.

- **A sub-agent is a published agent version referenced as a tool.** For
  Native, Goose, and OpenClaw agents it is declared in `guild.yaml` at the
  root of the version's files: "Each integration operation, sub-agent, and
  built-in tool you declare becomes a tool the agent can call."
  (https://docs.guild.ai/guide/goose-agents, "Integrations and sub-agents
  (`guild.yaml`)"). The literal block is two fields:

  ```yaml
  sub_agents:
    - name: acme~research
      version: ^1.0.0
  ```

  with the rules "Required. The sub-agent identifier, such as `acme~research`."
  and "Required. A semver range, such as `^1.0.0`, that must resolve to a
  published version." (same page, "Sub-agent fields"). Native and OpenClaw
  reuse the same surface: "The field rules and validation for `integrations`,
  `sub_agents`, and `builtins` are the same as for Goose agents"
  (https://docs.guild.ai/guide/native-agents,
  https://docs.guild.ai/guide/openclaw-agents).
- **In TypeScript the same idea is an exported helper.** `guildAgentTool`,
  described only as "Create a custom tool that dispatches to another Guild
  agent" (https://docs.guild.ai/packages/agents-sdk, "Utilities"). There is no
  `task.spawn` and no `task.subtask` symbol anywhere in the documentation set.
- **Creation is declared ahead of time and version-pinned at build.** "3.
  **Validate Guild subagents**" resolves "each sub-agent resolves to a
  published version." and "5. **Store Guild tools**" records "the resolved
  tool manifest is recorded on the version. Tool names must be unique across
  integrations, sub-agents, and builtins; a duplicate is a build error."
  (https://docs.guild.ai/guide/native-agents, "Build-time validation"). The
  visibility rules are enforced at the same moment: Guild rejects the build for
  "A private sub-agent or integration owned by another account.", "A private
  sub-agent or integration owned by the same account when the agent itself is
  public.", and "A sub-agent that has been archived."
  (https://docs.guild.ai/guide/versions, "Tool and dependency validation").
- **Auto-managed state agents must be compiled to call one.** "Calling a
  sub-agent or service hook from an uncompiled auto-managed state agent
  crashes at runtime. To prevent this, Guild validates at build time that any
  auto-managed state agent using these tools is compiled." The trigger list
  names "Sub-agents (tools of `toolType: "agent"`)." and the consequence is
  that "Saving or publishing an uncompiled auto-managed state agent that
  registers sub-agents or integration service hooks fails with a build
  validation error, which blocks the publish."
  (https://docs.guild.ai/guide/coded-agents, "Compilation required for
  sub-agents and service hooks").
- **Instantiation is one task record per invocation, in the parent's session.**
  "Returns `200` with a page of tasks, oldest first. The root task (the one
  your trigger started) has `parent_task_id: null`; tool calls and sub-agent
  runs appear as their own tasks with `parent_task_id` pointing back to the
  task that spawned them." (https://docs.guild.ai/platform/api-triggers, "Fetch
  session sub-tasks"). The discriminator is stated as "`EntTaskAgent` for the
  root agent task or a sub-agent invocation; `EntTaskTool` for a tool call.",
  so a sub-agent call and a tool call are the same kind of node in one tree.
- **Concurrency is a parent-side primitive, not a child-side one.**
  "`task.gather` runs an array of tool or sub-agent calls concurrently." and
  "Deferred calls" (the elided clause reads "such as sub-agent invocations")
  "are batched: the agent's state machine suspends, all deferred calls are
  dispatched together, and execution resumes once every call has settled." with
  "Results are always returned in source order."
  (https://docs.guild.ai/sdk/task-object).

### What a child inherits, and what is isolated

Shared with the parent, documented:

- **The session.** Both `TaskAgent` and `TaskTool` carry `session_id`, and the
  route is `GET /sessions/{session_id}/tasks`, so child tasks live inside the
  parent's session.
- **Workspace variables.** "`task.env` is always available. It holds the
  workspace's [variables](https://docs.guild.ai/platform/workspace-variables), resolved when the
  task dispatches or resumes." with "Values are stable within a turn and
  refreshed between turns, so an edit made mid-task takes effect on the next
  resume at the earliest." (https://docs.guild.ai/sdk/task-object).
- **LLM configuration.** "The provider and model are resolved at runtime from
  the **workspace owner's** [LLM settings](https://docs.guild.ai/platform/llm-settings), not in
  agent code." (https://docs.guild.ai/guide/llms, "Configuration").
- **Credentials, in the sense that nobody holds them.** "Agents never see
  credentials." and "**Service credentials** are injected server-side by
  Guild's credential proxy after a [credential
  policy](https://docs.guild.ai/platform/credential-policies) check. The credential never enters the
  agent's code, container, prompt, or state."
  (https://docs.guild.ai/platform/security-architecture, "Secretless
  execution").
- **A runtime container, conditionally.** "If set, only tasks in that specific
  session can use the runtime container, otherwise it can be shared within the
  workspace." (`RuntimeContainer.locked_for_session_id`,
  https://docs.guild.ai/api-reference/sessions/fetch-session-runtimes).

Isolated per child, documented:

- **State.** "State is scoped to the current task, and is retained across
  suspensions (e.g., while waiting for user input)."
  (https://docs.guild.ai/sdk/task-object), and `saved_state` is a per-task
  field on `TaskAgent`.
- **Agent version and tool manifest.** Each task carries its own `version_id`
  ("The version of the agent that this task is running. If None, it means
  we're running the assistant."), and a child's tools come from its own
  version's resolved manifest.
- **Token accounting.** Each `TaskAgent` carries its own `token_usage`,
  `input_tokens`, `output_tokens`, `llm_call_count`, `total_tokens`.
- **Filesystem.** A Native child has none at all: "Guild runs the agentic loop
  itself, so there is no container and no filesystem."
  (https://docs.guild.ai/guide/agent-types).

### Nesting, depth, and fan-out

Nesting is implied by `parent_task_id` and never bounded by a documented depth
number. The governing noun is the **execution tree**, and the caps sit on the
tree and on per-task fan-out. From https://docs.guild.ai/reference/limits
("Unlimited power mode" and "LLM execution budgets"), quoted cell by cell:

| Limit | Default | With unlimited power mode |
| --- | --- | --- |
| "Agent tasks per execution tree" | "50" | "250,000" |
| "Tool fan-out per task" | "300" | "250,000" |
| "LLM calls per execution tree" | "500" | "1,000,000" |
| "LLM tokens per execution tree" | "50,000,000" | "20,000,000,000" |

The framing is explicit that these are not tuning knobs: "Guild enforces hard
runtime limits that act as runaway backstops. They protect against infinite
loops, unbounded state growth, and runaway LLM billing costs." Enforcement is
at the proxy: "When a budget is exceeded, the LLM proxy returns HTTP `429 Too
Many Requests` and the call fails." The raised ceiling is qualified against
being read as a cost control: "The raised token limit is **not** a cost guard.
Cache tokens are excluded from the count, so billed spend can exceed it, and
20 billion input tokens is on the order of tens of thousands of dollars."

Four more documented execution limits bear on delegation. State: "The
serialized state has a maximum size of 8 MiB." with "The runtime enforces the
same 8 MiB ceiling on the `save-state` endpoint and returns HTTP `413 Request
Entity Too Large`" and "The runtime limit is authoritative." Synchronous
steps: "the runtime limits consecutive budget refreshes to **10**, which
corresponds to 10,000 synchronous steps without yielding.", not configurable
because "the ceiling is managed by the platform and is not configurable from
your account." Turn wall clock: "The default ceiling is 3,600 seconds (1
hour). When a turn exceeds the timeout, the runtime terminates the in-progress
command and completes the turn with a `TURN_TIMEOUT:` result that contains the
last-seen session ID. The calling agent resumes the container session with a
follow-up message instead of the run failing." And counting windows differ by
root: "Chat and agent-test roots count from the latest user message, so the
call and token budgets reset on each user turn. Trigger roots count across the
full execution tree for the entire run."

Guild's own guidance argues *against* a sub-agent layer added for concurrency:
"The same work over N inputs" "belongs in **one** compiled agent that maps
those inputs to tool calls in a single `task.gather`. Do not create a second
sub-agent just to gain concurrency: `task.gather` already runs the batch in
parallel inside the single agent."
(https://docs.guild.ai/guide/coded-agents, "One agent, one job"), restated as
"Running the same work over many inputs concurrently is not a reason to split
an agent in two." (https://docs.guild.ai/guide/agent-types).

### Parent and child communication

Structured input in, structured or text output back, plus a shared session and
a shared event log. Input: "Structured input from a parent agent is validated
against the Goose agent's input schema before any wrapping. When the input
already conforms to the schema, it passes through unchanged, so a parent agent
can send structured `parameters` directly. Only input that does not conform to
the schema is wrapped as canonical text `{"type": "text", "text":
"<serialized input>"}`." (https://docs.guild.ai/guide/goose-agents, "Chat
input"). Output: `task.gather` "has `Promise.all` semantics: if any call fails,
the entire batch rejects with that error.", while `task.gatherSettled` "never
rejects." Native and OpenClaw children are text-only: "Native agents have a
fixed text contract: they take text in and return text out." and the same
sentence for OpenClaw. For a multi-turn `llmAgent`, "The value the LLM passes
to `__submit__` becomes the agent's final output. Earlier turns in the session
are not returned to the caller."
(https://docs.guild.ai/guide/llm-agents).

Observability is shared even though state is not: the event log "records every
LLM call, tool invocation, sub-task spawn, error, and lifecycle transition
inside a session, in real time." and "The table nests each event under the task
that produced it, so a sub-task's events sit with that task instead of in one
flat run." (https://docs.guild.ai/platform/security-architecture;
https://docs.guild.ai/platform/sessions). Nothing in the docs describes a child
sending a message to its parent mid-run; `task.ui.notify` and
`task.ui.prompt` target the user and the session surface.

## Configuration surface (what, where, why)

Configuration lives in four distinct homes, and no page puts all of an agent's
configuration in one place.

| Home | Holds | Format |
| --- | --- | --- |
| Agent version files (git-backed) | system prompt, instructions, tools, sub-agents, builtins, environment pin, model fallback list, skills (OpenClaw), input and output schema | `PROMPT.md` / `AGENTS.md` / `recipe.yaml` / `agent.ts`, plus `guild.yaml`; `guild.json` is CLI-managed |
| Workspace | context document, variables, triggers, installed agents, spend budget, `unlimited_power_mode` | web UI and CLI |
| Account (user or organization) | service credentials, credential policies, LLM provider credentials, model policies, daily token limit, skills, environments | web UI and CLI |
| Public API | agent creation, LLM-agent configuration, skill versions, install, credential association | JSON over HTTP at `https://api.guild.ai/v1` |

**Model: three layers, and the agent is the weakest of them.** "LLM
configuration is stored on an **account** (your user account or an
organization), not on individual workspaces. Sessions in a workspace use the
LLM settings for that workspace's owner account."
(https://docs.guild.ai/platform/llm-settings). The rationale is stated
outright, and it is the clearest single "why" statement in the corpus: "Together
these settings act as a central model gateway: credentials are held
server-side and never distributed to agents, [model
policies](#model-policies) control which models each workspace and agent may
call, the [daily token limit](#daily-token-limit) caps spend, and every call is
attributed to its workspace, agent, and user in
[Insights](https://docs.guild.ai/insights/usage)." Policies are an allowlist ("a model is available
only if a matching rule allows it"), they resolve down "Account (root) ->
Workspace -> Agent -> Workspace agent", and "Only the matching level applies.
Policies from different levels are not merged." An agent may state a
preference but not win with it: "Preferences are **strict**, and the account's
[model policies](https://docs.guild.ai/platform/llm-settings#model-policies) have the final say: a
preferred model is used only if a policy allows it."
(https://docs.guild.ai/guide/llm-agents). Goose recipes cannot ask at all,
because "provider and model are platform-controlled.", and Native agents have
no per-agent preference knob.

**System prompt: a file in the version, per type.** `PROMPT.md` (Native),
`AGENTS.md` (OpenClaw), `instructions` in `recipe.yaml` (Goose), `systemPrompt`
in `agent.ts` (TypeScript LLM), `system_prompt` in the request body
(https://docs.guild.ai/api-reference/agents/configure-an-llm-agent). For
OpenClaw the surrounding workspace files are prompt configuration too, with a
stated precedence rule: "Every file you commit is placed in the agent's
workspace. `AGENTS.md` is the only required one; the rest are optional, and
**a committed file always wins**", where Guild "fills a gap but never overrides
what you ship." `IDENTITY.md` otherwise comes "Generated from the agent's
Guild record.", `USER.md` "Generated from the session's workspace context.",
`SOUL.md` "Gives the agent a distinct persona and voice.", and `MEMORY.md`
"Seeds the agent's memory on the first turn."

**Tools: declared, never inherited.** "Every tool a Native agent can call is
declared in `guild.yaml` at the root of the agent's version files."
(https://docs.guild.ai/guide/native-agents). Two rationales are stated. Context
budget: "Keeping the list short helps an agent stay under the tool limits
models impose, which a full integration can exceed on its own."
(https://docs.guild.ai/guide/goose-agents). And fail-loud configuration:
"Fields that Guild cannot honor are rejected at build time with an explicit
error rather than silently ignored" because "a recipe author who declares an
extension expects the agent to have it, and running without it would produce
confusing behavior."

**Permissions are not an agent-side list.** Guild exposes no per-agent
permission block on the definition. Authority is expressed as account-held
credential policies and model policies, optionally scoped to an agent, and
enforced outside the agent: "Because every outbound call passes through Guild,
policy is enforced at the point of egress rather than by agent code:"
(https://docs.guild.ai/platform/security-architecture).

**Credentials: the design centre, and the reason the agent definition is
thin.** "Agents never see credentials. When an agent calls a service tool (e.g.
`github_issues_get`), the request is routed through Guild's credential proxy,
which checks the [credential policies](https://docs.guild.ai/platform/credential-policies) for that
credential, agent, and workspace, then injects authentication server-side. The
credential never enters the agent's code, container, prompt, or state."
(https://docs.guild.ai/platform/credentials). Provider keys follow suit: "LLM
provider keys follow the same model: they are held server-side and never
placed in the agent runtime." GitHub is short-lived by construction: "**GitHub
access** uses a GitHub App with short-lived installation tokens minted on
demand, bounded to the repositories the App is installed on."

A credential policy is the fine-grained half, with its rationale in the second
sentence of the page: "Credential policies let you define fine-grained access
rules that the runtime enforces before any request reaches the external
service. Limiting what each agent can do with a credential reduces the impact
when something goes wrong." Evaluation is deny-biased: "If a matching `DENY`
rule exists, the runtime blocks the request regardless of any `ALLOW` rules.
If no rule matches, the runtime denies the request by default." Rules can bind
resources, not only operations: "GitHub | `repos` | `owner/repo` patterns,
e.g. `acme/*`", "Slack | `channels` | Channel IDs, e.g. `C0123ABCD`", "HTTP |
`domains` | Hostname patterns, e.g. `*.internal.acme.com`", "Any | `methods` |
HTTP verbs: `GET`, `POST`, `DELETE`, ...", and "Use `methods` to separate read
access from mutating access."

One qualification deserves flagging, because the platform's own default
contradicts its stated posture: "Guild auto-creates an unscoped allow-all
policy so a credential works before you write any rules." and "Delete the
allow-all policy once your scoped rules cover everything agents should reach.
Until you do, the fallback is allow, not the platform's default deny." The
default-deny claim made everywhere else is therefore true of the evaluation
rule and false of the shipped starting state.

**Outbound network: no route at all, stated as the reason integrations
exist.** "Agents have no direct route to the internet. An agent's container can
reach the Guild API and nothing else, so every outbound request" travels
"through Guild." with the practical consequence that "`fetch` exists but cannot
connect, and `axios` and `node:http` cannot be imported in the first place:"
(https://docs.guild.ai/guide/sdk-introduction, "Network isolation"). The rule
has no exemption for the customer's own abstraction layer: "This applies to
every line of agent code, including inside custom tools built with
`guildServiceTool` or `guildAgentTool`. A `fetch` call inside a custom tool
fails exactly as an inline one does." Containers flip posture mid-life:
"**Setup phase.** The container has network access to install dependencies and
prepare the workspace." then "**Runtime phase.** Before agent execution
begins, the container moves to an offline network with no general egress. From
this point, the only outbound paths are Guild-mediated proxies."
(https://docs.guild.ai/platform/security-architecture).

**Environment: an account-level template, pinned per version.** "An environment
is a reusable template for where an agent runs: a base container image plus an
optional setup script that prepares the container before the agent starts.
Define an environment once and any number of agent versions can run in it."
(https://docs.guild.ai/platform/environments), which is the stated reusability
rationale. It is testable before anything depends on it: "**Test setup** checks
a setup script before an agent depends on it. It boots a throwaway container
from the environment, runs the setup script, and reports whether it
completed". Not every type can pin one: "The `environment` field is not
supported for Native agents" and "The `environment` field is not supported for
OpenClaw agents."

**Workspace variables: the explicit statement of why configuration lives
outside the agent.** "Store workspace-level configuration outside agent code so
agents stay reusable." and "By extracting settings like a repository name or a
Slack channel out of an agent and into the workspace, you can reuse the same
agent across different workspaces and customer accounts without editing its
code." (https://docs.guild.ai/platform/workspace-variables). They are
deliberately not secrets: "Workspace variables are not designed for
credentials, API keys, passwords, or other secrets. Their values can appear in
chat logs and model instructions." Prompt references are checked at build:
"Keys must match `[A-Z][A-Z0-9_]*`; a malformed reference is a build error, so
a typo surfaces at save time rather than as a literal ..." (the elided literal
is the unsubstituted `env` placeholder itself, which this page cannot reproduce
because the site renderer would read it as a template expression)
"in the model's context." (https://docs.guild.ai/guide/native-agents).

**Context versus skills: always-on versus on-demand, with the contrast
stated.** Workspace context "is a text document that Guild passes to every
agent when it runs in your workspace." and "Context is injected verbatim into
the agent's prompt", with the warning "**Keep it focused.** Long or noisy
context degrades agent performance." (https://docs.guild.ai/platform/context).
Skills are the other half: "Unlike [workspace context](https://docs.guild.ai/platform/context),
which is always included, skills are **activated on demand**. The agent sees a
catalog of available skills and their descriptions, and decides which ones to
pull in based on the current task." The rationale is named: "This follows a
progressive disclosure model: the agent sees a lightweight catalog for
discovery, then loads the full instructions only when needed." And the boundary
is explicit: "Skills are **knowledge-only**. A skill can instruct an agent on
how to use tools it already has, but it cannot add new tools, grant
capabilities, or execute code. Think of skills as reference documents, not
plugins." (https://docs.guild.ai/platform/skills).

**Triggers and schedules: workspace-level, three kinds.** "A trigger runs an
agent automatically" (https://docs.guild.ai/platform/triggers). Event triggers
are `webhook` on the wire, schedule triggers are `time`, and API triggers fire
only on request: "Unlike event and schedule triggers, an API trigger doesn't
fire on its own". Each API trigger carries its own credential: "Every API
trigger is authenticated by a trigger API key: a combined
`<api_key_id>:<api_key_secret>` credential generated when you create the
trigger. Each key is scoped to that one trigger." Sessions started that way
have no human behind them: "**Execution Identity:** Sessions started via an API
key do not run on behalf of a human user (`acting_user_id` is `None`)."
Deactivation is the documented pause: "Deactivate a trigger to pause it without
deleting it."

**Spend budgets are the one deliberately opt-in money control.** "Spend budgets
are a different mechanism" "opt-in, set by an admin, denominated in USD, and
measured across a calendar month. They can be configured at three scopes:
workspace, agent, and user." (https://docs.guild.ai/reference/limits), with
"Spend accrues after each call completes, so a scope can overshoot its ceiling
by roughly one in-flight call before the block takes effect."
(https://docs.guild.ai/platform/workspaces).

## Binding time

Guild binds configuration at five distinguishable moments. The clearest single
statement of the model is per-task prompt immutability: "The system prompt is
written once, at the task's first dispatch, and is immutable for that task's
lifetime. Editing `PROMPT.md` therefore applies to tasks created after the next
build, not to a session already in flight."
(https://docs.guild.ai/guide/native-agents, "The prompt").

| Moment | What binds here |
| --- | --- |
| Author time (local files) | prompt text, tool declarations, sub-agent semver ranges, environment pin, model fallback list, input and output schema |
| Save and build (version commit) | dependency resolution, integration and sub-agent version resolution, tool manifest, `env_references` extraction, schema generation, visibility rules |
| Publish | availability for install; version number derived |
| Install into a workspace | which version a workspace runs; the workspace's variables, context, and triggers become the agent's surroundings |
| Session or task dispatch | system prompt materialization, <code v-pre>{{env.KEY}}</code> substitution, container creation and setup script, LLM policy resolution, workspace context injection |
| Per tool call, mid-run | credential resolution and injection, credential policy evaluation, model policy and spend checks |

**At save and build.** "When you save an agent with `guild agent save`, Guild
validates the tools and dependencies declared in the build before persisting
the version. If a declared dependency is broken or violates visibility rules,
Guild rejects the build with a `400 BadRequest` error and lists the specific
validation failures." (https://docs.guild.ai/guide/versions). Dependency
reproducibility is checked here too: "For TypeScript agents, `guild agent save`
validates `package-lock.json` so dependency installs stay reproducible."
(https://docs.guild.ai/cli/getting-started). Goose reports everything at once
and persists the derived contract: "On success, three values are persisted on
the agent version: `description`, `input_schema`, and `output_schema`."
(https://docs.guild.ai/guide/goose-agents). Native and Goose skip the step
entirely: both "skip the build validation step and immediately transition to
`READY` after initialization." (https://docs.guild.ai/cli/getting-started).

**At publish.** "When you publish with `guild agent save --publish`, Guild
derives the next version number from the latest published version." with
`--bump major` giving `2.0.0`, `--bump minor` giving `1.3.0`, and `--bump
patch` (the default) giving `1.2.4` from `1.2.3`; "When nothing has been
published yet, the first version is `1.0.0` regardless of bump level."
(https://docs.guild.ai/guide/versions). Publishing is synchronous by
construction: "`--publish` implies `--wait`, so it always waits for validation
and publish to finish before reporting success." with a 300 second default
timeout.

**At install.** The install pins `version_id`, and `should_autoupdate`
defaulting to `true` is what re-points it on each publish ("If True, the agent
will automatically update when a new version is published."). Updating a
published agent is never a mutation: "To update an agent, save a new version
and publish it:" (https://docs.guild.ai/platform/publish-to-agent-hub).

**At dispatch.** The system prompt materializes and freezes (quoted above).
The OpenClaw workspace is rebuilt: "The workspace is rebuilt for each task from
the version's committed files, and versions are immutable, so the agent can
write to its workspace freely without affecting later tasks."
(https://docs.guild.ai/guide/openclaw-agents). Variables substitute, context
injects ("is injected into the first user message of a session only." for
Native), LLM configuration resolves ("At runtime, Guild resolves LLM
configuration for each session from the workspace owner's account settings."),
the setup script runs "during container setup, before the agent starts", and
trigger input is validated, with dispatch failure recorded rather than left
hanging: "When Guild cannot dispatch a triggered run to the agent, it records
the failure and moves the task to an error state, rather than leaving it
pending indefinitely."

**Per tool call.** This is the layer Guild re-resolves on every request, and it
is what makes mid-run revocation work: "because credentials are resolved at the
proxy on every request, subsequent tool calls are denied" "including calls from
sessions that are already running."
(https://docs.guild.ai/platform/credentials). Model policy edits are likewise
immediate ("Changes apply immediately."), and spend is checked "before an LLM
call".

### Can a definition change while instances are running

Three answers, consistent with each other.

- **Version content: no.** Editing the prompt "applies to tasks created after
  the next build, not to a session already in flight." Immutability is asserted
  as the premise for that: "Because versions are immutable, a recipe that
  validated at build time parses identically at task start."
  (https://docs.guild.ai/platform/goose-recipes). The driver layer repeats the
  pattern: "A session's instructions are fixed when it is created, so passing
  them again would be silently ignored."
  (https://docs.guild.ai/guide/codex-driver).
- **Account and workspace policy: yes, immediately, including for running
  sessions.** "**Revoke a credential.** [Disconnecting a
  credential](https://docs.guild.ai/platform/credentials#managing-credentials) immediately denies
  subsequent tool calls, including from sessions already running."
  (https://docs.guild.ai/platform/security-architecture, "Stop controls").
- **Agent existence: yes, and it stops in-flight runs.** "Archiving an agent
  removes it from every workspace it is installed in and stops it from
  running." (https://docs.guild.ai/platform/agents). Operator moderation
  reaches further: "`DISABLED` and `TAKEN_DOWN` are refused at run dispatch, so
  existing installs stop working, not just new ones."
  (https://docs.guild.ai/platform/publish-to-agent-hub).

Stopping a session is terminal rather than pausing: "Stopping a session
immediately transitions its running tasks to **Interrupted**, records who
stopped the session and when, and writes the interruption to the session's
[event log](#event-log)." and "Interrupted sessions cannot be resumed."
(https://docs.guild.ai/platform/sessions).

Four things carry independent versions: agent versions (semver, derived at
publish), skill versions ("Each version is immutable", with `@1.2.3` pinning
and a bare name resolving to latest), workspace context versions ("Every change
creates a new version. Versions are either `DRAFT` or `PUBLISHED`"), and
integration versions resolved into an agent version by semver range at build.

## Relationships between nouns

Cardinalities, from the OpenAPI component schemas and the platform pages:

| Noun | Schema or route | Parent | Cardinality |
| --- | --- | --- | --- |
| Account | `Account` = `oneOf` `Organization`, `User` | none | owns agents, workspaces, skills, credentials |
| Workspace | `Workspace` | `owner_id`, `creator_id` | 1 account to N workspaces |
| Workspace agent (the install) | `WorkspaceAgent`, `/workspace_agents/{id}` | `workspace_id` plus `version_id` | 1 workspace to N installs; an install pins one **version** |
| Agent | `Agent` | `owner_id` | 1 account to N agents; 1 agent to N installs (`installs_count`) |
| Agent version | `AgentVersion` = `oneOf` committed, ephemeral | `agent_id`, `author_id` | 1 agent to N versions |
| Environment | `runtime_environment_id` on both version schemas | account-owned template | 1 environment to N versions; optional (`nullable: true`) |
| Runtime container | `RuntimeContainer`, `GET /sessions/{id}/runtimes` | `workspace_id` | belongs to a workspace, not to an agent |
| Session | `Session` = `oneOf` five variants | `workspace_id` on every variant | 1 workspace to N sessions |
| Task (sub-task) | `Task` = `oneOf` `TaskAgent`, `TaskTool` | `session_id`, `parent_task_id` | 1 session to N tasks, forming a tree |
| Event | `Event` = `oneOf` nineteen variants | `task_id` on every variant | 1 task to N events |
| Skill | `Skill`, `SkillVersion` | `owner_id` | account-scoped; no workspace or agent key |
| Credential association | `CredentialAssociation` | `target_id` | binds one credential to one **install** |
| Trigger | `trigger_id` on three session variants | workspace | 1 trigger to N sessions |

The containment chain: `Account` owns `Workspace` owns `Session` owns `Task`
owns `Event`; `Account` owns `Agent` owns `AgentVersion`; `WorkspaceAgent` is
the join that puts one `AgentVersion` into one `Workspace`;
`RuntimeContainer` hangs off the `Workspace` and a `Task` points at one
optionally ("An optional runtime, if the agent is not run in the default Guild
runtime.").

**Agent to session: one agent, many sessions, and the session does not own the
agent.** The prose says "A session is a conversation with an agent.", but
`components.schemas.SessionChat` has **no** `agent_id`. Its only agent-shaped
field is `assistant_version_id`, "The assistant version resolved for this chat
session. Null for legacy sessions and direct agent sessions that did not use
the assistant." The agent binding lives one level down, on the task
(`TaskAgent.version_id`). Which agent answers is therefore resolved per
message, not per session, and chat exposes that: "In workspace chat, type `@`
followed by an agent name to direct your message to a specific agent." with
"If no agent is mentioned, the message goes to the default agent for the
workspace (Smith, unless changed)." Only the test variant pins a version for a
whole session: `SessionAgentTest.version_override_id` is "The specific agent
version being tested in this session."

**Session types are five, and API keys may create only one.** `Session` is
`oneOf` `SessionAgentTest`, `SessionChat`, `SessionTriggerApi`,
`SessionTriggerTime`, `SessionTriggerWebhook`. On the create route: "`chat` is
the only `session_type` a key may create" and "`time`, `webhook`,
`api_trigger`, and `agent_test` sessions are a `403`."
(https://docs.guild.ai/api-reference/workspaces/start-a-session-in-a-workspace).
The request schema pins it structurally, with `session_type` as an enum of one
value described as "The only value a key may pass."

**Agent to environment, and to a container.** An environment is referenced by a
*version*, not by an agent, and a container belongs to a workspace rather than
to either. An agent does not imply an environment: "No code and no container"
for Native agents (https://docs.guild.ai/platform/agents), against "Goose or
OpenClaw when the agent needs a container". `RuntimeContainer.base_path` is the
most direct statement of who drives execution: "Guildcore will call
`<host>/<base_path>/<start|resume>`" (backticks added here so the site renderer
does not read the placeholders as markup; the schema text carries none).

**Skills sit outside the agent and do not travel with it.** "Skills are
**account-scoped**" and "Publishing an agent does not bundle its skills for
other organizations. If another organization installs your agent, that
organization also needs the relevant skills created or imported into its own
account for the agent to discover and activate them."
(https://docs.guild.ai/platform/skills).

**Lifetime coupling for sub-agents is documented only at session granularity.**
"Any running session can be [stopped](https://docs.guild.ai/platform/sessions#stopping-a-session)
from the UI or API. Running tasks are halted and the interruption is recorded
with who stopped it and when."
(https://docs.guild.ai/platform/security-architecture). What happens to an
in-flight child when the parent task alone errors is not stated.

### Authorization shape

Scopes are `(group, access)` pairs: "A key's access is limited to the scopes
you grant it when you create it." with `agents`, `sessions`, `workspaces`,
`skills`, and `integrations` as groups, and "`access` is `read` or `write`;
`write` implies `read`. A key created with no scopes can authenticate but
reaches nothing." (https://docs.guild.ai/api-reference/introduction). Scope is
a ceiling, not a grant: "A scope only grants access to entities the key's
account can already see or own. It never lets a key reach another account's
private data, and it can't act as a person".

**Denied reads are indistinguishable from absent ones.** "The resource doesn't
exist, or exists but the key can't see it" and "Guild returns 404 rather than
403 for reads, so a denied read is indistinguishable from an absent one. On a
write, a 404 usually means an id in the path or parameters doesn't match
anything the key can see" (same page, "Response conventions"). The pattern
repeats per route: a private agent "needs `agents:read` on a key belonging to
its owner account, or the read 404s" and "a denied read looks identical to a
nonexistent agent."; a restricted workspace "is invisible to every account key
regardless of scope, and returns `404`.". One documented deviation: "A key
missing `agents:read` currently gets a `500`, not a filtered `200` or a `403`"
because "serializing an agent task reads details the key isn't allowed to see,
and that read fails loudly instead of being omitted."

**Publishing publicly is admin-only, in two places.** For agents: "an
`agents:write` key can create and publish agents, but only an admin can make
one public." For skills: "A key cannot create a *public* skill even with
`skills:write`" because "publishing is admin-only." Credentials, spend budgets,
and audit logs are admin-gated too: "Admins can connect and disconnect
credentials for organization-owned integrations."; "Budgets are admin-only.";
"Only Admin users can view audit logs." The one carve-out is stated as such:
"One exception: installing a workspace agent that needs its own dedicated
credential lets the installer mint that agent's key without being an admin."

Containment for API keys is tight and deliberately uninformative: "A key
converses only in the chat sessions it initiated. Posting into any other
session" "is `404`, not `403`: a key cannot inject messages into a human's
conversation, and cannot even learn that the other session exists.", while
"Reading is broader than writing".

Audit coverage is asserted as tamper-evident three times ("a tamper-evident,
exportable record of administrative actions") and no page states a mechanism:
no hash chain, signature scheme, append-only store, or external anchoring is
described, and the only supporting statement is "Audit log entries are
read-only and cannot be modified or deleted." Read it as a claim rather than a
documented construction. Scope is stated precisely, which helps: "Audit logs
record every attempt to change something through the Guild API in your
organization" "successful or not." and "Read-only requests are not logged, and
neither are internal runtime and operator traffic."

## Lifecycle

**Agent.** Created by `POST /agents` (requires `agents:write`) or by
`guild agent init`, moving through `CREATED`, `GIT_REPOSITORY_CREATED`,
`READY`. Then per iteration: save creates a version, validation runs (or is
skipped by type), publish makes it installable, install pins it into a
workspace. There is also a no-code write path: "Requires `agents:write`. Writes
a new committed version of an LLM agent from a system prompt, description, and
tool list."
(https://docs.guild.ai/api-reference/agents/configure-an-llm-agent).

**Destruction is archiving, not deletion.** "Archiving an agent removes it from
every workspace it is installed in and stops it from running. Archived agents
are excluded from agent listings, and new sessions cannot target them. No user
can install or fork an archived agent" (including its publisher). Moderation is
a separate operator-held axis: `moderation_state` in `[ACTIVE, DEMOTED,
UNLISTED, DISABLED, TAKEN_DOWN]`, "Operator-controlled moderation/curation
state. Owners cannot change this; only operators can". The wider kill switches
are workspace archiving, which "**deactivates every trigger** in the
workspace" and, notably, "this is the part that outlives the archive, since
unarchiving does not reactivate them.", and organization deletion, which is out
of band: an admin request that "notifies Guild support, who processes the
deletion manually." No public API delete route exists for an agent, a version,
or a session; the only `DELETE` in the OpenAPI document is on a credential
association.

**Session.** Start over the API is one call that also begins execution ("the
agent begins executing `initial_prompt` immediately."). States are **Active**
("The agent is running or waiting for your input"), **Completed**, **Failed**,
and **Interrupted** ("A user stopped the session; running tasks were halted").
Multi-turn sessions end on an explicit tool call: "The session remains active
until the agent calls the `__submit__` tool to signal completion." Task states
are finer: `CREATED`, `DISPATCHED`, `STARTED`, `RUNNING`, `WAITING`, `ERROR`,
`DONE`, `INTERRUPTED`.

**Pause and resume are the runtime's job, not the author's.** Suspension is a
compiler feature for auto-managed state: "`babel-plugin-agent-compiler` is an
internal Babel plugin used by the Guild runtime to translate procedural
TypeScript agent code into state machines that can be paused, serialized, and
resumed." with three generated methods, `step` ("Runs the state machine until
it reaches an `await` expression, then returns the pending `Promise`"), `get`
("Serializes the entire state machine state as a JavaScript object for
storage"), and `set` ("Restores a previously serialized state"). Resumption
survives infrastructure churn: agents can be "resumed later" "even after a
runtime restart." Self-managed state agents do the same thing by hand through
`task.save()` and `task.restore()` and "do **not** use the `"use agent"`
directive."

**Who owns the loop: Guild does, for every documented agent type.** "Guild runs
the loop itself, so nothing has to start before the agent's first turn."
(https://docs.guild.ai/guide/agent-types), and the control plane calls `start`
and `resume` against the runtime's web server
(`RuntimeContainer.base_path`). The single place the customer owns a loop is a
hand-rolled ReAct loop over `task.llm`, framed explicitly as the non-default:
"`generateText` makes one model call and returns. There is no `maxSteps` and no
`stopWhen`, so there is no built-in ReAct loop." and "If you would rather not
write the loop, [`llmAgent`](https://docs.guild.ai/guide/llm-agents) is the managed version".
Third-party engines are drivers inside Guild's runtime rather than an escape
hatch: "Guild's runtime can drive agent turns with the OpenAI Codex coding
engine. The Codex driver runs alongside the existing Claude driver, so the
runtime can execute either engine in the same environment." and "You do not set
these values yourself" "the runtime manages them for every turn."
(https://docs.guild.ai/guide/codex-driver).

**Where execution physically happens: Guild-managed infrastructure.** "Agent
code runs in Guild's runtime, a separate service from the control plane. The
runtime holds no credentials and no policy state", requesting both "from the
control plane per operation." No bring-your-own-loop or self-hosted-runtime
surface appears anywhere in the fetched pages.

**What persists across runs.** Task state as a row field (`saved_state`, capped
at 8 MiB); the durable event log, but not live drafts ("Both are stored as
ephemeral drafts rather than as durable event rows." and "Draft events carry
synthetic, ephemeral IDs and are not valid cursors."); artifacts on the session
("an artifact persists on the session and can be fetched, listed, and shared on
its own."); and cross-run configuration outside the agent, in workspace
variables ("Agents read these values dynamically when they run, so the same
agent behaves correctly in each workspace it is installed in."). Filesystem
persistence is not documented as durable anywhere: `RuntimeContainer` has a
`destroyed_at`, and for OpenClaw "The workspace is rebuilt for each task from
the version's committed files".

## What makes it "an agent" here (our inference)

Our inference: in Guild an agent is **an owned, git-backed configuration record
whose immutable published versions are executed by Guild's own loop under
account-held policy**. The definition names its input and output types, its
prompt or code, and the tools it may ask for; it never holds a credential, a
model choice it can guarantee, a network route, or the loop that runs it. What
separates it from a plain LLM call is not autonomy: it is that the thing is
addressable (`owner/name` plus a UUID), installable into a workspace at a
pinned version, budgeted per execution tree, and observable as a task tree in a
session event log, with every outbound effect crossing a proxy that re-checks
policy on each request.

Two design commitments follow, and both are the opposite of a thick agent
definition. First, **the definition is deliberately unprivileged**: the same
sentence appears in three forms across the docs, that credentials never enter
"the agent's code, container, prompt, or state", that model preferences are
"strict" but model policies "have the final say", and that skills "cannot add
new tools, grant capabilities, or execute code". An agent version can ask for
things; only the account can grant them, and only the proxy can serve them.
Second, **failures are pushed to build time on purpose**, and the docs argue the
case each time rather than just asserting it: an unsupported section is "a build
error rather than being silently ignored"; a malformed variable reference
surfaces "at save time rather than as a literal ..." placeholder "in the
model's context"; a skill directory with no `SKILL.md` is rejected because otherwise
"the skill is silently never discovered"; and a rejected recipe field is
rejected because "running without it would produce confusing behavior". The
determinism actually delivered is narrow: immutable versions, a resolved tool
manifest recorded on the version, semver pinning of sub-agents to published
versions, and a per-task system prompt frozen at first dispatch. It does not
extend to the model, which "Changes apply immediately" at the account level, or
to credentials, which are re-resolved per request by design. Worth noting
against the reading that Guild sells determinism: the docs never apply the word
to the platform. It attaches to one agent type ("Build deterministic TypeScript
agents with automatic state management.",
https://docs.guild.ai/guide/coded-agents) and to one evaluation check ("the
deterministic outputMatches lexical check",
https://docs.guild.ai/reference/evals). The platform-level claim is control,
not determinism: "The control plane for AI agents."
(https://docs.guild.ai/index, "Introduction").

Worth carrying into our own work: the two places Guild's claims and its
defaults disagree. The platform describes a default-deny credential posture and
ships an "unscoped allow-all policy" until an operator deletes it; and it
describes "tamper-evident" audit logs with no stated mechanism beyond being
read-only.

## Open questions

- **`LANGGRAPH`.** The API enums carry `agent_type: LANGGRAPH` and
  `AgentCreationTemplate: LANGGRAPH`, but no page in the index documents a
  LangGraph agent type, its files, or its validation. Whether it is shipped,
  internal, or vestigial is not stated.
- **Which TypeScript flavor the runtime dispatches, and how.** The docs never
  say whether `llmAgent`, auto-managed, and self-managed are distinguishable on
  the record after creation; `creation_template` is explicitly not
  authoritative ("it doesn't mean the agent is still following the template
  now"). How the runtime chooses at dispatch is documented only implicitly, via
  the default export's shape.
- **Immutability of versions in general.** The word "immutable" is applied to
  versions only on the Goose and OpenClaw pages, each time as the premise for a
  local argument. No general statement says a committed version's files cannot
  change, and `AgentVersionCommitted` has an `updated_at` the docs do not
  explain.
- **`force_publish`.** A field on `POST /versions/{version_id}/publish` with
  `default: false` and no page saying what it forces or bypasses.
- **Agent `status` beyond `READY`.** `CREATED` and `GIT_REPOSITORY_CREATED`
  appear only in the schema; no prose mentions them, and no failure state is
  documented.
- **Draft as two different things.** "**Draft**: no published version" is
  agent-level; "Unpublishing moves the version from `PUBLISHED` back to
  `DRAFT`" is version-level. The docs do not reconcile the two usages, and
  unlike `validation_status` the version-status enum is never given.
- **Sub-agent nesting depth.** No page states a maximum depth. "Agent tasks per
  execution tree" (50 by default) bounds the total count, which bounds depth
  only indirectly, and nesting itself is never explicitly confirmed or
  forbidden.
- **`guildAgentTool`'s signature.** Documented only by its one-line
  description. No parameters, return type, or example call appears anywhere.
- **Sub-agent tool naming and arity.** The only evidence is CLI example output
  showing "Sub-agent: guildai/triage-agent" with the tool "run_triage". No page
  states the rule that turns an `owner~name` into a tool name, nor whether a
  sub-agent exposes exactly one tool or several.
- **The parent's call site for a sub-agent.** The Goose page documents the
  child's view of incoming input, but no page shows the parent's call
  expression or the shape of the value it receives back.
- **Credential and variable scoping for a child owned by a different account.**
  Credential associations target a workspace-agent install and `task.env` is
  workspace-wide, but no page states whether a sub-agent's tool calls resolve
  credentials against the parent's install or the child's.
- **Lifetime coupling below the session.** Stopping a session halts its tasks.
  Nothing states what happens to an in-flight child task if the parent alone
  errors or is interrupted, nor what `INTERRUPTED` means for a child
  specifically.
- **Whether `task.gather` counts against "Tool fan-out per task" (300).** The
  limit name matches, but no page connects the batch size to that cap.
- **In-place upgrade of an install.** `should_autoupdate` exists in the schema
  and no prose page explains it, nor what happens to a running session when
  autoupdate re-points the install, nor how to install a specific older version
  through the UI.
- **Uninstall over REST.** The audit log records agent "uninstall" and the MCP
  server exposes `guild_remove_workspace_agent`, but the OpenAPI document has
  no corresponding `DELETE`.
- **`sessions:read`.** The scope table says the `sessions` group covers reading
  session status, events, tasks, and runtimes, yet every session read route
  documents `workspaces:read` plus `agents:read`. The two sources contradict
  each other.
- **Filesystem persistence across runs.** `RuntimeContainer` has a
  `destroyed_at` and unlocked containers "can be shared within the workspace",
  but no page states whether a coding agent's working tree survives between
  sessions or between resumes, nor who decides to reuse versus create, nor an
  idle or eviction rule.
- **Trigger, environment, eval, and artifact as API resources.** Each is a
  first-class product noun with no schema or route in the public OpenAPI
  document, so their own fields are unquotable. Evals surface only as
  `test_type: EVAL` on an agent-test session plus a spec document whose
  storage, numbering, and immutability are undocumented, and the two eval pages
  disagree on whether there are three or four check types.
- **Sandbox resource limits.** No page documents CPU, memory, or disk ceilings
  on an agent's container, only the 3,600 second turn timeout and the 8 MiB
  state cap.
- **Memory as a first-class thing.** `MEMORY.md` for OpenClaw and `task.save()`
  for self-managed state are the only mechanisms. There is no documented
  account- or workspace-level memory store, and no retention or eviction
  policy.
- **`unlimited_power_mode` authorization.** The docs say to turn it on from the
  workspace settings but never say which role may do so, unlike spend budgets
  which are explicitly admin-only.
- **`init` callback and `task.baseurl`.** `init` is named among the callbacks
  that receive a `Task` with no page documenting what it is for; `task.baseurl`
  is referenced in the Codex driver's base URL template but is absent from the
  Task object's member tables.
- **Retrieval of prior doc states.** No archive or snapshot URL and no content
  digest is published, so every record here rests on the local copies plus the
  retrieval date.
