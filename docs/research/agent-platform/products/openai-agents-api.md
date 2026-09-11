# OpenAI Agents API: what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Researched 2026-09-11, one day after the 2026-09-10 public-beta
announcement. Evidence from the announcement post, the
`developers.openai.com/api/docs/guides/agents-api/*` guide set (overview,
architecture, configuration, sessions, sessions/events, sessions/manage,
sessions/webhooks, multi-agent, environments/lifecycle, observability,
quickstart), and the OpenAI developer-forum announcement thread. Claims
sourced only from a third-party walkthrough are marked **(secondary)**; the
Agents API reference pages themselves were not retrievable as raw text and
are noted under Open questions.

This dossier is a companion to, not a replacement for,
[OpenAI Agents SDK + AgentKit](./openai-agents-sdk.md). The two products
coexist and make opposite choices, which is the reason the Agents API earns
its own entry.

## Why this product reopens a settled finding

The SDK dossier recorded OpenAI as the corpus's clearest case of a vendor
walking *away* from the server-side agent resource: it "had the versioned
server-side agent resource everyone else is building (Assistants), and
retired it in favor of code objects + a versioned server-side Prompt +
server-side Conversations, decomposing 'agent' into config, state, and loop
rather than keeping it one resource."

The Agents API reverses the loop half of that decomposition and restores a
server-side agent record. OpenAI now ships both postures at once, and says
so plainly in its own comparison: for the Agents SDK the loop runs "Inside
your application"; for the Agents API OpenAI runs it. The corpus reading
that OpenAI is a one-way move toward agent-as-code-object no longer holds.

## The `agent` noun (primary-source quotes)

- Overview, the four-concept model: "The Agents API is built around four
  main concepts: **Agent**: The model, instructions, tools, and MCP servers
  available to the agent. **Environment**: An optional sandbox or computer
  where the agent accesses files, loads skills, and runs commands.
  **Session**: A durable instance of an agent that works on tasks and
  responds to input. **Events and items**: The inputs sent to an agent and
  the output produced during a session."
- Announcement framing: "Build and run cloud agents with the Codex harness,
  fully managed by OpenAI."
- Overview: "The Agents API gives your application access to the Codex
  harness through an OpenAI-managed API. OpenAI manages sessions,
  orchestration, context compaction, and recovery while your application
  provides tools and chooses its execution environment."
- Configuration: "An agent configuration defines how the agent behaves. You
  can supply it when creating a session or save it for reuse. The session
  holds the conversation and work, while the saved agent holds reusable
  settings."
- The agent is therefore **two things at once**, and the docs keep them
  distinct by name:
  - an **inline configuration**, the `agent` object on `POST
    /v1/agents/sessions`, with no identity and no lifetime beyond the
    session that carries it;
  - a **saved resource**, minted by `client.beta.agents.create(...)`,
    addressed later as `agent_id` on session creation. The reference "lists,
    retrieves, updates, or deletes saved agents."
- Conceptual model: **agent-as-config, optionally promoted to
  agent-as-record**, with the loop and the state deliberately not part of it.
  The agent record is pure reusable settings; everything durable about a run
  lives on the session.

## Subagents

- Named **subagents**, with a **coordinator** (also "main agent" or
  "root agent") above them. Enabled by one flag, not by declaration:
  "Set `agent.multi_agent.enabled` to `true` when you create a session. The
  harness supplies tools to create, message, wait for, and interrupt
  subagents. You do not declare these tools yourself."
- **Creation is dynamic, by the model, at runtime.** There is no roster and
  no registry. The four harness-supplied coordination actions surface as
  item types: `create_subagent_call`, `send_subagent_input_call`,
  `wait_for_subagents_call`, `interrupt_subagent_call`.
- **Fan-out limit:** `max_concurrent_subagents`, "The default is `6`,
  excluding the coordinator." Nesting depth is not documented; see Open
  questions.
- **Inheritance, stated exactly:** "Subagents inherit configured MCP tools,
  their credentials and allowed tools, and web search settings. They can
  also use the environment's files and command-line tools. Subagents do not
  support function tools."
  The function-tool exclusion has a structural cause, not a policy one: a
  function call pauses the session for the *application* to answer, and the
  docs route required actions through the session, so a subagent's function
  call would have no addressable answering path.
- **Isolation is context only.** "Each subagent has its own context and can
  work in parallel with the others." Everything else is shared: "The
  coordinator and subagents share its filesystem. Creating a subagent does
  not create another environment."
- **A subagent is not a session.** It is an actor *inside* one session,
  identified by `subagent_id`, which appears on turns: "The turn's
  `subagent_id` is `null` for the main agent." Each subagent nonetheless
  keeps "its own item history and a per-turn items endpoint."
- **Communication:** shared filesystem plus explicit message passing
  (`send_subagent_input_call`) plus a join (`wait_for_subagents_call`).
  Results are not structurally returned: "A completed create or wait action
  does not mean the subagent finished its task... Read the main agent's
  response for the combined result." An `agent_message` item "contains
  inter-agent text when available, but the stream does not provide a full
  conversation transcript."
- **Coordination of writes is left to the prompt:** "Agents that edit the
  same files must coordinate their changes."

## The `environment` noun

Worth its own section because no other dossier in the corpus has a
first-class, separately-identified, reconnectable compute resource beside
the session.

- Three types, chosen at session creation: `none`, `openai_hosted`,
  `self_hosted`.
- Architecture names the three parties: "**Harness**: The OpenAI-hosted
  Codex instance that runs the model and tool loop and maintains the agent's
  session. **Environment**: Where the agent runs commands, executes code,
  and works with files. An environment can be a remote sandbox, your laptop,
  a Docker container, or an AWS Lambda function. **Application server**:
  Your code that connects the agent to your product."
- `none` costs capability: "Without an environment, the built-in Bash and
  apply-patch tools, workspace files, and executor MCPs are unavailable."
  It also forces eager input: "Sessions with `environment.type: 'none'`
  require initial input."
- The environment has **its own id, its own status, and its own event
  family**, separate from the session's: `agent.session.environment.pending`,
  `.connected`, `.disconnected`, `.failed`, plus a status that moves from
  `provisioning` to `connected` **(secondary)**.
- **Lifetimes are explicitly decoupled:** "An agent session can outlive its
  environment." And deletion does not cascade: "Delete the session and stop
  provider compute separately. Deleting a session neither stops its
  environment nor emits a deletion webhook."
- **Cardinality is 1:1 per session, not per subagent:** "Every session
  receives a different environment ID and needs a separate executor."
- `openai_hosted` configuration knobs: `packages` (python / npm / system),
  `setup_commands` (ordered, each with its own `cwd`, nonzero exit blocks
  the agent from starting), `files` (inline base64 or Files API id), `env`,
  `network.access` (`enabled` default / `disabled` / `restricted` with 1 to
  100 exact hostnames, no wildcards, ports, or paths),
  `environment_template_id`, `capability_directories`, `workspace_directory`
  **(secondary for the per-field detail; the field names appear in the
  guides)**.
- Two knobs carry stated security rationale: `env` "rejects `PATH`, anything
  starting with `CODEX_`, and `OPENAI_API_KEY`", and a template-derived
  session "can't broaden the network policy" **(secondary)**.
- `self_hosted` inverts who connects. The customer runs `codex exec-server`
  from the Codex CLI, registers through `https://api.openai.com`, then holds
  a WebSocket to `wss://codex-cloud-environments.chatgpt.com`; both
  connections originate from the customer's side **(secondary)**. The
  guides' own framing: "Your code starts the environment and connects an
  executor to the session... Your application manages the connection and
  lifecycle without forwarding each command."
- Self-hosted credentials are deliberately split: an **environment key**
  (`CODEX_API_KEY`) whose only permission is connecting environments, kept
  separate from the application key, because "Generated code can read this
  key" **(secondary)**.

## Configuration surface (what, where, why)

- **On the agent:** `model`, `instructions`, `tools`, `reasoning`
  (for example `{summary: "auto"}`), output format and detail, and
  `multi_agent: {enabled, max_concurrent_subagents}`. Configuration guide:
  "Model: Which model does the work. Instructions: What the agent should do
  and how it should behave. Tools: What actions the agent can take...
  Reasoning and output: How much reasoning the model uses and the format and
  detail of its responses."
- **Tool types observed in examples:** `function` (JSON Schema, answered by
  the application), `mcp` (with `server_label`, `transport`, `required`,
  `allowed_tools`, `connection_origin`), `web_search`,
  `programmatic_tool_calling`, and `tool_search`.
- **Two token-efficiency knobs with stated rationale.** Tool search "loads
  relevant tool definitions as needed, helping reduce token usage and cost
  while preserving the model's cache", paired with per-tool
  `defer_loading: true` **(secondary)**. Programmatic tool calling "lets
  agents run calls in parallel, chain related operations, and filter or
  combine results in code so they can work through large volumes of data
  while bringing only the relevant results back into context."
- **MCP connection origin is a first-class axis**, because the connecting
  party determines both reachability and which secrets apply: `http` with
  `connection_origin: "service"` (default, OpenAI connects, no environment
  needed), `http` with `connection_origin: "environment"` (the sandbox
  connects, environment required), and `stdio` (a process inside the
  sandbox) **(secondary for the table; the fields appear in the guides)**.
- **Credentials are a separate plane.** "Credentials stay in vaults,
  separate from the saved configuration." Sessions attach them by
  `vault_ids`. Inline HTTP `authorization` and `headers` are accepted but
  "encrypt[ed] and remove[d] from the session resource it returns", and
  vaults "only work for connections made from OpenAI" **(secondary)**.
  Stdio servers instead read named variables via `transport.env_vars`, with
  the docs warning to "Treat those values as exposed because sandboxed code
  can read them" **(secondary)**.
- **Where configuration lives:** the API only. There is no file format, no
  repo convention, and no dashboard-authored definition in the guides. This
  is the opposite of the Claude Code / AGENTS.md end of the corpus, and it
  is a clean inversion of the same vendor's SDK, where configuration is
  Python or TypeScript.

## Binding time

- **Session creation is the binding moment for everything.** Multi-agent:
  "These settings apply at session creation. Changes to a stored agent apply
  to new sessions." So a saved agent is resolved into the session once, and
  in-flight sessions are unaffected by later edits to their agent.
- **Per-session override, replace not merge:** "Include both `agent_id` and
  `agent` to customize a session that uses a saved agent. The session
  inherits omitted settings, including the model... Supplied objects and
  arrays replace the entire field rather than merging with the saved value.
  For example, supplying `tools` replaces the saved tool list." And:
  "Overrides apply only to that session. They do not change the saved agent
  or other sessions."
- **Mid-run mutability is limited to input, not configuration.** The one
  mutable-mid-run affordance is steering: "A message sent during an active
  turn steers that turn." Nothing in the guides mutates `agent`, `tools`, or
  `environment` on a live session.
- **No agent versioning.** The saved agent supports update and delete, with
  no revision number, no pinning, and no statement about what an update does
  to sessions already created from it beyond "apply to new sessions". This
  is a notable gap against Bedrock AgentCore, Claude Managed Agents,
  LangGraph assistants, and OpenComputer, all of which version the
  definition and pin it per session.
- **The harness itself is the versioned artifact instead.** The
  announcement: "The Agents API provides versioned access to these
  capabilities with each model launch. We maintain and continuously improve
  the harness alongside our models." The stated rationale is that harness
  rework is the tax being removed: "Taking advantage of new model
  capabilities often means reworking your harness, taking valuable time away
  from improving your application." The beta pins through a header,
  `OpenAI-Beta: agents=v1`.

## Relationships between nouns

Cardinalities as the guides state them:

- **Agent : Session, 1:N.** "Each session has its own conversation and
  work." A session may also carry no saved agent at all (inline `agent`).
- **Session : Environment, 1:1, independent lifetimes.** The session is
  durable, the environment is not, deletion does not cascade either way.
- **Session : Turn, 1:N.** "A turn is one cycle of work within a session. A
  message sent to an idle session starts a new turn."
- **Turn : Subagent, N:1, nullable.** `turn.subagent_id` is the attribution
  field; `null` means the root agent ran it.
- **Session : Subagent, 1:N, contained.** A subagent has no session of its
  own and no environment of its own.
- **Item : Turn, N:1.** Items carry `turn_id`; "For a root-agent turn,
  filter session items by `turn_id`."
- **Event : Item, N:1 by `item_id`.** "Items and events share `item_id`, so
  you can match them" **(secondary)**; the guides give the finer join keys
  `item_id`, `output_index`, `content_index`.

The distinction the docs repeat most is **event versus item**: "Events
report what happens as an agent works. Items are the saved messages and tool
calls you can retrieve later. Use events to update your application in real
time and items to display its saved history." Events are explicitly not a
log: "Streams do not replay missed events." Recovery is therefore a
documented five-step procedure (open a new stream and buffer, retrieve the
session and saved items, restore state by item id, apply buffered updates,
resume), not a resumable cursor.

## Lifecycle

- **Create:** `POST /v1/agents/sessions` with `agent` or `agent_id`,
  `environment`, and usually `input`. Optional `stream: true` streams the
  first turn from the same request.
- **Run:** turns are asynchronous. "Turns run asynchronously. Your
  application can follow progress through streaming or receive session state
  changes through webhooks."
- **Pause for the customer:** a session enters status `requires_action`,
  with `required_actions` carrying entries of exactly two kinds:
  `function_call` ("Run the function identified by `name` with its
  `arguments`. Return the result on the same session using the action's
  `turn_id` and `call_id`") and `environment_connection` ("Connect the
  environment identified by `environment_id`").
- **Steer or continue:** the same input channel does both, discriminated by
  session state. Cancel is also an input event
  (`agent.session.input.cancel`), and it is scoped to the turn: "Cancel the
  current turn when you want the agent to stop. The session and its previous
  work remain available."
- **Resume:** listed as a harness capability, "Resuming a session where it
  left off", alongside "Summarizing previous work to manage its context
  window."
- **Destroy:** `DELETE` the session; "Deletion removes the session from the
  API. Physical cleanup may continue asynchronously." Compute is a separate
  teardown. A hosted sandbox is reclaimed "after one hour without activity
  or keep-alives", and deletion during setup or execution returns `409`
  **(secondary)**.
- **Who owns the loop: OpenAI.** Fully managed, with the customer owning
  only tool implementations, environment lifecycle where self-hosted, and
  the reaction to events. Put in the SDK dossier's terms, this is the
  managed-loop position the Agents SDK explicitly is not.
- **What persists:** the session's conversation, items, and turns persist by
  default. The filesystem persists only while the sandbox does. Artifacts
  outlive both: "Anything the agent writes under `/workspace/outputs`
  becomes an immutable artifact when the turn completes. Those copies stay
  downloadable after the sandbox is gone" **(secondary)**.

## Observability, usage, and their stated limits

Three disclaimers are unusually explicit and are the most directly useful
finding for our own contract, because each one names a fact we already
record and they do not.

- **Usage is not authoritative.** "Session and turn resources expose
  best-effort `usage`. It can be `null` when unknown, and recorded counts
  may change as accounting arrives. Missing usage does not mean zero usage.
  These counts are not a final bill."
- **Idle does not mean success**, repeated across four separate pages:
  "`agent.session.idle` means the session is ready for more input, not that
  its last turn succeeded... A completed turn can still contain failed tool
  calls."
- **Truncation is unreported.** "Command-output truncation is not reported"
  and "The customer API does not indicate whether command output was
  truncated."

Traces are dashboard-only: "Trace retrieval and external trace exporters are
not part of the public beta API... Dashboard trace endpoints require
separate access and are not a supported customer API."

## Data handling

Overview, stated without hedging: "The Agents API currently supports data
residency only in the United States and does not support Zero Data Retention
(ZDR). Choosing a self-hosted sandbox does not make the Agents API
ZDR-eligible."

This is the residency posture of the harness, not of the compute, and it is
the reason the self-hosted environment does not buy the data guarantee a
reader might expect it to buy.

## Commercial shape

"There are no additional fees for using the Agents API, you simply pay for
the tokens and tools your agents use." Hosted sandboxes bill separately at
container rates, per 20-minute session per container by size, with
per-minute billing and a five-minute minimum for eligible sessions
**(secondary)**. The harness is given away; the tokens and the compute are
the product.

## What makes it "an agent" here (our inference)

Our inference: for the Agents API an agent is **a named, reusable
configuration that a durable server-side session instantiates and a
vendor-owned loop executes**, where the three things that make it an agent
rather than a chat completion are all owned by the vendor: the loop, the
context management, and the delegation. The customer keeps only the tools,
the data, and optionally the compute.

The sharper observation for our purposes is that OpenAI split the noun
across **three resources with three different lifetimes** (agent = settings,
session = durable state, environment = disposable compute) and then wrote
down the decoupling explicitly, including the cases where it surprises
people: sessions outlive environments, deletion does not cascade, subagents
get a context but not a filesystem or an environment. That decomposition,
not the harness, is the transferable design content.

## Open questions

- **The reference pages were not retrievable as raw text.** Exact request
  and response schemas, the full `agent` field list with types and accepted
  values, status enum spellings, and error shapes come from guide prose and
  code samples rather than the reference. Field-level claims marked
  **(secondary)** should be re-verified against the reference or an SDK's
  generated types before any contract work depends on them.
- **Subagent nesting depth is undocumented.** Whether a subagent may itself
  delegate, and whether `max_concurrent_subagents` is global to the session
  or per coordinator, is not stated.
- **Saved-agent update semantics under load.** "Changes to a stored agent
  apply to new sessions" says what happens to new sessions and is silent on
  whether the update is atomic, whether the prior configuration is
  retrievable, and whether anything records which configuration a given
  session resolved.
- **Turn identity across steering.** A steering message joins an active
  turn; whether it creates an item with the existing `turn_id` and how a
  reader distinguishes the opening input from a steer is not stated.
- **Item type catalog is partial.** The guides name
  `create_subagent_call`, `send_subagent_input_call`,
  `wait_for_subagents_call`, `interrupt_subagent_call`, `agent_message`,
  command items, and function-call items, without a complete enumeration.
- **The open-source claim is narrower than it reads.** "The Agents API is
  powered by the open-source Codex harness, giving developers visibility
  into the core logic." Visibility, not portability: nothing states that the
  public `openai/codex` harness is the same build, at the same version, as
  the hosted one.
- **AgentKit / Agent Builder / ChatKit** remain uncaptured, carried over
  unresolved from the [SDK dossier](./openai-agents-sdk.md).
