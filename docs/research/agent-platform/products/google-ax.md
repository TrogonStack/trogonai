---
title: "Google Agent Executor (AX): what 'agent' means"
source_urls:
  - https://github.com/google/ax
  - https://github.com/google/ax/tree/703a79f2a55def5be183ad7bd54da7c38cc22cc5
  - https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/README.md
  - https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto
  - https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/content.proto
  - https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/controller.go
  - https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/eventlog/eventlog.go
  - https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/registry.go
  - https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/server/server.go
  - https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/server/interceptors.go
  - https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/harness/harness.go
  - https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/harness/substrate/substrate.go
  - https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/config/config.go
  - https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/skills/skills.go
  - https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/ax.yaml
  - https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/cmd/ax/agentconfig.go
  - https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/python/antigravity/harness_server.py
retrieved: 2026-09-15
status: done
---

# Google Agent Executor (AX): what "agent" means

Part of [Agent platform research corpus](../index.md).
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence from the `google/ax` source: the README, the two `.proto` files that
constitute the entire wire contract, the controller and event log, the harness
interfaces, and the Python harness sidecar.

AX is Google's open-source distributed agent runtime. It is the runtime layer
above [Agent Substrate](https://github.com/agent-substrate/substrate), which
this corpus already touches from the other side in
[kagent](./kagent.md).

> **Naming.** `google/ax` is unrelated to `ax.ai` (an open-source agent
> framework) and to `facebook/Ax` (adaptive experimentation). This dossier
> always means `google/ax`, "Agent Executor".

## Source anchors

Sources retrieved 2026-09-15. Source-level claims are pinned to the
[v0.2.3 release](https://github.com/google/ax/releases/tag/v0.2.3)
(published 2026-08-13) at commit
[`703a79f`](https://github.com/google/ax/commit/703a79f2a55def5be183ad7bd54da7c38cc22cc5).
The repository has no documentation site: the README is the only prose
source, and at retrieval time it is byte-identical on the pinned tag and on
`main`. The default branch has since advanced to
[`b777313`](https://github.com/google/ax/commit/b77731302075b3630b200af5e2cf63ac93b5f315)
(2026-08-20); where the two could diverge this dossier prefers the pinned tag.

The repository is small enough to be read exhaustively: 102 files at the pin,
of which the wire contract is two files totalling 402 lines.

- Prose: [README](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/README.md).
- Wire contract: [`proto/ax.proto`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto)
  and [`proto/content.proto`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/content.proto).
- Controller: [`controller.go`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/controller.go),
  [`registry.go`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/registry.go),
  [`eventlog.go`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/eventlog/eventlog.go).
- Execution boundary: [`harness.go`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/harness/harness.go),
  [`substrate.go`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/harness/substrate/substrate.go).
- Configuration: [`config.go`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/config/config.go),
  [`ax.yaml`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/ax.yaml).
- Skills: [`skills.go`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/skills/skills.go),
  [`examples/skills`](https://github.com/google/ax/tree/703a79f2a55def5be183ad7bd54da7c38cc22cc5/examples/skills).
- Reference harness: [`harness_server.py`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/python/antigravity/harness_server.py).

The README carries a standing stability warning: "AX is in active early
development… We are actively refining our core, resumption protocols, and
runtime specifications, which will introduce major breaking changes prior to
a stable release," alongside a temporary policy pausing external pull
requests
([README:3-10](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/README.md#L3-L10)).
`ax.proto` repeats it: "This file is in active development and
significant changes can be made with breaking changes"
([`ax.proto:24-25`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto#L24-L25)).
Every finding below should be read against that warning.

## The `agent` noun (primary-source quotes)

- **The README never defines an agent.** It defines the product against
  agents: "AX, short for Agent Executor, is a distributed harness runtime. It
  dynamically provisions isolated environments from suspendable/resumable
  images to execute harnesses and agents"
  ([README:12-14](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/README.md#L12-L14)).
  The noun appears in feature bullets ("Harnesses, skills, tools, and agents
  can execute in isolation",
  [README:20](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/README.md#L20))
  and in a motivating claim that the industry is "moving away from
  monolithic agents towards distributed harnesses where tools, skills and
  agents are deployed as isolated actors"
  ([README:67-70](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/README.md#L67-L70)).
  None of these is an operational definition.

- **There is no agent resource.** The wire contract declares exactly two
  services, `HarnessService`
  ([`ax.proto:85-91`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto#L85-L91))
  and `InteractionsService`
  ([`ax.proto:125-129`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto#L125-L129)),
  and neither has a create, read, update, delete, list, or version method for
  an agent. There is no `Agent` message anywhere in either file.

- **What exists instead is a pair of fields on a request.**
  `CreateInteractionEvent` carries `string agent_id = 4`, commented "Agent
  ID, empty selects the default agent", plus `bytes agent_config = 5`,
  commented "Per-request agent configuration (opaque JSON), if any"
  ([`ax.proto:111-118`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto#L111-L118)).
  The agent is therefore an identifier plus a blob supplied per call, not a
  stored object.

- **`agent_id` is resolved as a harness id.** The controller assigns
  `req.AgentId` directly into `harnessID`, falls back to the conversation's
  recorded value and then to `d.registry.defaultHarness`, and looks the result
  up in the harness registry
  ([`controller.go:84-100`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/controller.go#L84-L100)).
  The same value is written back into the event log as `StepEvent.agent_id`
  ([`controller.go:256-264`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/controller.go#L256-L264)).
  The source records this as unfinished rather than intended: "TODO(anj): We
  need to consolidate agents and harness registration. Adding harness
  registration support temporarily"
  ([`controller.go:74-75`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/controller.go#L74-L75)).

- **Conceptual model: agent-as-request-parameter.** Not agent-as-identity,
  not agent-as-config as a resource, and not agent-as-process, since the
  process-shaped noun is the harness. The closest honest description is that
  "agent" in AX names whichever harness a caller selects, plus the JSON that
  caller hands it on that call. The distinction is narrow but load-bearing:
  the config is *recorded*, since `LogInputs` parses valid `agent_config`
  into `StepEvent.agent_config` and appends it to the event log
  ([`controller.go:243-264`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/controller.go#L243-L264)),
  yet nothing reads it back. `ResumptionState` recovers only `state` and
  `agent_id`, so the stored copy is an audit annotation on one turn rather
  than a definition the runtime resolves.

The absence is unusual in this corpus not because a product declines to store
an agent, but because here it coexists with a fully specified server-side
execution resource: AX models the run in detail and the actor not at all.

## The `harness` noun

The harness, not the agent, is the product's load-bearing noun, and unlike the
agent it is precisely specified in three places.

- **As a wire contract.** `HarnessService.Connect` is a bidirectional stream:
  "The client sends `HarnessRequest{start}` (and may send one
  `HarnessRequest{cancel}` mid-stream), and the server streams zero or more
  `HarnessResponse{outputs}` frames terminated by exactly one
  `HarnessResponse{end}`"
  ([`ax.proto:86-90`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto#L86-L90)).
  Implementing that one method is the whole extension surface: "you can bring
  your own harness implementation by implementing `HarnessService`"
  ([README:246-249](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/README.md#L246-L249)).

- **As a Go interface pair.** `Harness` has a single method, `Start(ctx,
  conversationID, config) (Execution, error)`, where "config carries optional
  per-request configuration; it is opaque to the controller and interpreted by
  the harness implementation"
  ([`harness.go:42-47`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/harness/harness.go#L42-L47)).
  `Execution` adds `Run`, `Queue`, `ID`, `Close`
  ([`harness.go:49-63`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/harness/harness.go#L49-L63)).

- **As a registry entry.** `Registry` is an in-process map with
  `RegisterHarness`, `Harness`, and `SetDefaultHarness`
  ([`registry.go:25-77`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/registry.go#L25-L77)),
  populated from the YAML config file at process start. There is no
  registration API and no registry persistence.

`RegistryConfig` splits harnesses into "Built-in harnesses (e.g. Antigravity,
AntigravityInteractions) whose implementation and container image are provided
by AX" and "Custom harnesses on substrate whose implementation and container
image are provided by the user via their own ActorTemplate"
([`config.go:93-102`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/config/config.go#L93-L102)).
Harness ids are reserved constants, `antigravity` and
`antigravity-interactions`
([`config.go:35-37`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/config/config.go#L35-L37)).

## Subagents

**There are none.** No message, field, service method, or config key in the
pinned tree expresses a child agent, delegation, handoff, spawn, or fan-out.
`Step` has four arms (content, thought, tool call, tool result;
[`ax.proto:134-145`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto#L134-L145))
and none of them references another conversation.

The absence is worth recording precisely because the README's stated
motivation is distribution: "we are moving away from monolithic agents towards
distributed harnesses where tools, skills and agents are deployed as isolated
actors"
([README:67-70](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/README.md#L67-L70)).
What is distributed is *hosting*, one isolated actor per conversation, not
*delegation*. A harness that internally calls another agent would do so
through its own MCP or A2A client, invisibly to AX, and AX would record the
result as an ordinary `ToolResultStep`.

## Configuration surface (what, where, why)

Configuration splits cleanly into two planes, and only one of them is AX's.

**Deployment plane: `ax.yaml`, read once at process start**
([`config.go:46-59`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/config/config.go#L46-L59)):
server address; event log backend (`sqlite` filename or `postgres` DSN,
[`config.go:77-91`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/config/config.go#L77-L91));
the harness registry; a skills block; and OTLP telemetry. The checked-in
sample is 20 substantive lines
([`ax.yaml:18-34`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/ax.yaml#L18-L34)).

Skills are deliberately placed here rather than on an agent, and the stated
reason is harness-independence: the block "makes agent skills available on
disk before the harness starts. It is harness-agnostic: each actor runs
exactly one harness, which consumes the resulting folder(s)"
([`config.go:53-58`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/config/config.go#L53-L58)).
The resulting shape is purely positional: a `Group` "associates a set of
skills with the directory that contains them. Each skill lives at
`<Dir>/<skill-id>/` (with a SKILL.md inside)"
([`skills.go:23-28`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/skills/skills.go#L23-L28)).
Two sources exist, a local directory and the Gemini Enterprise Skill Registry
([`skills.go:15-21`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/skills/skills.go#L15-L21)).
There is no version, digest, or pin on a skill.

**Behavior plane: `agent_config`, opaque to AX.** The controller never
interprets it; it forwards the bytes and, separately, best-effort parses a
copy into a `structpb.Struct` purely so the log is readable, downgrading a
parse failure to a warning
([`controller.go:243-255`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/controller.go#L243-L255)).
Its schema therefore belongs to whichever harness receives it. In the
reference Python harness the schema is the SDK's `LocalAgentConfig` Pydantic
model, and the request payload is validated as an *overlay* on the sidecar's
startup default, top-level keys only: "nested-key and value/type validation is
delegated to the SDK's own LocalAgentConfig validation"
([`harness_server.py:141-149`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/python/antigravity/harness_server.py#L141-L149)).

**Why each knob sits where it does** is mostly unstated, with one explicit
exception and one explicit gap. The exception is skills, justified above by
harness-agnosticism. The gap is credentials, and the source names it: a
`_NON_AGENT_CONFIG_FIELDS` set currently excludes only `conversation_id` and
`save_dir`, under "TODO: add validation for fields that are unsafe to set per
execution (e.g. credentials, deployment routing) or that may only be set at
conversation creation"
([`harness_server.py:39-45`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/python/antigravity/harness_server.py#L39-L45)).

**Credentials are environment variables.** The README documents
`GEMINI_API_KEY` for AI Studio, and Application Default Credentials plus
`GOOGLE_CLOUD_PROJECT`, `GOOGLE_CLOUD_LOCATION`, `GOOGLE_GENAI_USE_VERTEXAI`
for Vertex ([README:111-129](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/README.md#L111-L129)).
There is no secret resource, no credential binding, and no brokered egress.

**AX itself ships no authentication or authorization.** The overview diagram
labels the server "AX Server (multi-tenant)"
([README:44-49](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/README.md#L44-L49)),
but the only interceptors AX installs are `LoggingInterceptor` and
`StreamLoggingInterceptor`
([`interceptors.go:30-79`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/server/interceptors.go#L30-L79)),
and they only log. Scope that claim to the in-process server: `Serve` appends
its own chain to caller-supplied `grpc.ServerOption` values
([`server.go:75-87`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/server/server.go#L75-L87)),
so an embedder can add credentials or an interceptor of their own, and
nothing here says anything about a deployment's transport or ingress. What
the pinned tree does show is that no tenant identifier reaches the
controller, the registry, or the event log, so multi-tenancy is a label on
the diagram rather than a control AX implements.

## Binding time

- **Harness set and default: process start.** The registry is built from
  `ax.yaml` and never mutated afterwards
  ([`registry.go:32-77`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/registry.go#L32-L77)).
  Changing the set of harnesses means restarting the server.
- **Harness selection: first interaction of a conversation, then frozen.**
  The controller derives the stored harness id from the log and refuses a
  change: "resumption not allowed: harness ID changed from %s to %s"
  ([`controller.go:82-88`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/controller.go#L82-L88)).
  This is the single strongest pin in the product, and it pins a *name*, not
  a version or a digest.
- **`agent_config`: every request, freely.** It rides on
  `CreateInteractionEvent` and again on each `HarnessStart`
  ([`ax.proto:39-43`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto#L39-L43)),
  and the CLI ships an interactive `/config` menu that edits it mid-
  conversation, noting "An updated config is sent on subsequent requests"
  ([`agentconfig.go:28-30`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/cmd/ax/agentconfig.go#L28-L30)).
  Model, system instructions, and skill paths can therefore change between
  turns of one conversation with no record beyond the log line.
- **Versioning: none.** There is no version, revision, digest, or activation
  concept attached to an agent, a harness selection, or a skill anywhere in
  the pinned tree. The harness id resolves through a mutable in-process map,
  so restarting the server with a different template behind the same id
  changes what that id means with nothing recorded about the change. The
  blast radius is narrower than that sounds, and AX does not decide it:
  `Start` calls `CreateActor` and tolerates `AlreadyExists` without
  re-applying a template
  ([`substrate.go:87-96`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/harness/substrate/substrate.go#L87-L96)),
  so whether an existing conversation keeps its original image is a property
  of Agent Substrate, which is outside this pin. What AX does determine is
  that a conversation whose actor is created after the change gets the new
  template under the old id, and no artifact in AX records which one ran.

## Relationships between nouns

The nouns are conversation, interaction, execution, step, harness, agent id,
actor, skill.

- **Conversation → interaction → step.** "A conversation is the historical
  session that consist of a number of execution. A conversation cannot be
  continued before the last execution is completed or failed"
  ([`ax.proto:27-29`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto#L27-L29)).
  `StepEvent` carries both `conversation_id` and `interaction_id` and a
  repeated `steps`
  ([`ax.proto:30-37`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto#L30-L37)).
  The interaction is not a resource: a `TODO(jbd)` records the intent,
  "CreateInteraction should return an Interaction message and the outputs
  should be polled from the Interaction"
  ([`ax.proto:131-132`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto#L131-L132)).
- **Conversation ↔ actor is 1:1, and the conversation id *is* the actor
  name.** `SubstrateHarness.Start` calls `CreateActor(ctx, conversationID)`,
  tolerating `AlreadyExists` because "on follow-up turns the actor was created
  (and suspended) on a previous turn", then `ResumeActor`, then dials the
  returned worker pod IP and health-gates for up to 60 seconds before handing
  back the execution
  ([`substrate.go:87-123`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/harness/substrate/substrate.go#L87-L123)).
- **Conversation ↔ harness is many:1 and fixed after the first interaction**
  (see Binding time).
- **Agent ↔ harness collapses to identity.** `agent_id` is looked up in the
  harness registry; there is no separate agent namespace.
- **Skill ↔ agent does not exist.** Skills attach to the process's config and
  reach the harness as a directory path, so every conversation served by that
  process sees the same skills.
- **Tool ↔ anything is invisible to AX.** MCP is a harness-side concern; the
  controller only ever sees a `ToolCallStep` and a matching `ToolResultStep`
  correlated by `call_id`
  ([`ax.proto:165-196`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto#L165-L196)).
  There is no admission check: nothing in AX can reject a tool the caller
  never authorized.

## Lifecycle

- **Who owns the loop: the harness, not AX.** AX drives turns, not
  reasoning. `Exec` starts a harness execution, queues inputs, runs one turn
  to completion, and closes
  ([`controller.go:64-145`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/controller.go#L64-L145)).
  The agentic loop itself lives inside the harness.
- **States are four, on the execution.** `STATE_PENDING`, `STATE_FAILED`,
  `STATE_COMPLETED`, `STATE_CANCELED`
  ([`ax.proto:93-101`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto#L93-L101)).
  There is no state on a conversation, and no archive, expire, or delete
  operation on anything.
- **Cancellation is typed but coarse.** `CancelReason` is
  `USER_REQUESTED`, `TIMEOUT`, `INTERNAL_ERROR`
  ([`ax.proto:103-109`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto#L103-L109)),
  delivered as one mid-stream `HarnessCancel`.
- **Suspend and resume belong to the compute layer.** Between turns the
  Substrate actor is suspended and its state snapshotted; the next turn
  resumes it. The README is explicit that this is platform-conditional:
  "Advanced Resumption: Support for compute-layer actor resumption on
  compatible platforms"
  ([README:29](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/README.md#L29)).
- **What persists across runs**: the event log persists the transcript; the
  actor's filesystem and in-process state persist in the snapshot; nothing
  else. There is no memory resource.

## Durability and resumption: what the event log is and is not

The README's two durability claims, "Single-Writer Architecture: Single
controller ensures consistent state management" and "Event Log: Durable
execution state with automatic recovery"
([README:27-28](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/README.md#L27-L28))
are worth reading against the interfaces, because the mechanism is narrower
than the phrasing suggests.

- **The log's interface is three methods**: `Append(ctx, *StepEvent) (int64,
  error)`, `Events(ctx, conversationID) ([]*StepEvent, error)`, `Close()`
  ([`eventlog.go:31-40`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/eventlog/eventlog.go#L31-L40)).
  There is no cursor, no pagination, no read-from-sequence, and **no write
  precondition**: nothing in the interface can express "append only if the
  log is still at the position I read". Backends are SQLite and Postgres,
  rows encoded with `protojson`.
- **Single-writer is an assumed invariant, not an enforced one.** It is
  documented as an obligation on the caller: "Single-writer expectation: the
  controller must ensure that at most one Execution exists per conversation
  id at a time. Harness implementations rely on this invariant -- for
  example, a harness that durably persists per-conversation state may use a
  last-write-wins store without compare-and-swap, which is correct only
  because there is a single writer per conversation"
  ([`harness.go:36-41`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/harness/harness.go#L36-L41)).
  No fence, lease, or compare-and-swap implements it at the pin.
- **The controller can break its own invariant.** On a `STATE_PENDING`
  conversation with non-empty inputs, `Exec` starts an execution, runs it,
  and defers `Close` to the end of the *function*; if inputs remain it then
  calls `h.Start` again for the same conversation id while the first
  execution is still open
  ([`controller.go:108-132`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/controller.go#L108-L132)).
  Two `Execution` values for one conversation therefore overlap on exactly
  the resume path, which is the path the single-writer comment exists to
  protect. The invariant is the intent; the implementation does not yet hold
  it, and because the log has no precondition nothing downstream notices.
- **The log is not the recovery mechanism.** `ResumptionState` reads the
  entire conversation only to derive two scalars: the last non-unspecified
  `state`, and the first non-empty `agent_id`
  ([`controller.go:224-241`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/controller.go#L224-L241)).
  The recorded steps are never replayed into a harness. `HarnessStart` has a
  `repeated Step steps` field
  ([`ax.proto:39-43`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/proto/ax.proto#L39-L43)),
  but every call site populates it with the *new* inputs for the turn, not
  with history
  ([`substrate.go:208-218`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/harness/substrate/substrate.go#L208-L218)).
  Conversation state therefore lives inside the harness actor's snapshot;
  the log is an audit record plus a resumability flag. The eventlog comment
  describing replay, "replaying the log in order brings the executor back to
  a consistent state from which execution can resume"
  ([`eventlog.go:28-30`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/eventlog/eventlog.go#L28-L30))
  describes an intent that no code path at this pin performs.
- **Resume is partially built.** `Exec` opens with "TODO(jbd): Resume an
  incomplete execution if there exists one"
  ([`controller.go:72`](https://github.com/google/ax/blob/703a79f2a55def5be183ad7bd54da7c38cc22cc5/internal/controller/controller.go#L72)),
  while the code below it does restart a `STATE_PENDING` execution against
  the recorded harness.

One consequence for modelling: because usable history is the actor's and not
the log's, a lost or evicted snapshot leaves the durable transcript intact
but leaves no path back to a running conversation. The record survives; the
harness state cannot be resumed or reconstituted from the log alone.

## What makes it "an agent" here (our inference)

*Our inference, not a product claim.*

In AX, nothing is an agent. The unit the product actually models is a
**conversation bound to one harness and hosted in one resumable isolated
actor**; "agent" is the name of the string that selects that harness and the
JSON handed to it. AX takes the position that the agent is whatever the
harness implements, and that a runtime's job is to give that implementation a
durable identity, an isolated place to run, and a transcript, while
declining to model behavior, capability, or authority at all.

That is a coherent position, and it is the mirror image of the products in
this corpus that model an agent richly and leave execution to the customer. It
also means the axes this corpus cares about most, what a definition
contains, when it binds, and what a version covers, are not answered by AX so
much as declared out of scope.

## Open questions

- **Does an agent resource arrive, and does it absorb the harness id?** The
  source says consolidation is intended ("we need to consolidate agents and
  harness registration") without saying which noun survives.
- **Does the event log gain a write precondition?** Without one, the
  single-writer invariant has no enforcement point, and the README's
  "multi-tenant" server has no way to fence a duplicate controller.
- **Will replay become real?** The eventlog contract promises replay-based
  recovery that no code path performs; whether the log or the snapshot is
  meant to be authoritative for history is unresolved, and the two answers
  imply very different products.
- **Where do credentials land?** The Python harness marks per-execution
  credential and routing fields as needing validation that does not exist
  yet. Nothing indicates whether the eventual owner is AX, the harness, or
  the compute layer.
- **What does multi-tenancy mean here?** The diagram asserts it; no
  authentication, authorization, or tenant identifier exists at the pin.
- **Is `interaction_id` intended to become addressable?** The `TODO(jbd)` on
  `CreateInteraction` suggests a polling model that would change the client
  contract substantially.
