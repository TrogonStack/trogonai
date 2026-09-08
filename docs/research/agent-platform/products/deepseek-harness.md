---
title: "DeepSeek Harness: what 'agent' means"
source_urls:
  - https://github.com/deepseek-ai/deepseek-harness
  - https://github.com/deepseek-ai/deepseek-harness/tree/141eb6fef83422698aef7a981029e843e8161534
  - https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/architecture.md
  - https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent/README.md
  - https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent-loop/README.md
  - https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/README.md
  - https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/preset/agent-presets/README.md
  - https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/system-prompt/README.md
  - https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/README.md
retrieved: 2026-08-20
status: done
---

# DeepSeek Harness: what "agent" means

Part of [Agent platform research corpus](../index.md).
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).

All repository evidence is fixed to Git commit
[`141eb6fef83422698aef7a981029e843e8161534`](https://github.com/deepseek-ai/deepseek-harness/tree/141eb6fef83422698aef7a981029e843e8161534),
which tag
[`dsh-v0.1.0-rc.8`](https://github.com/deepseek-ai/deepseek-harness/tree/dsh-v0.1.0-rc.8)
resolved to at retrieval. The commit is the auditable source snapshot for every
quote and code claim below.

## The `agent` noun (primary-source quotes)

The project-level definition is:

> "DeepSeek Harness (`dsh`) is an open-source agent harness developed by
> DeepSeek AI."

Source: [`README.md:5`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/README.md#L5).

The core interface package describes its own noun as:

> "Agent interface, registry, process-local initiator scope, and `agent/*`
> event vocabulary."

Source: [`packages/core/agent/README.md:5`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent/README.md#L5).

The type contract makes the identity rule explicit:

> "The single identity shared with session."

Source: [`packages/core/agent/src/runtime-types.ts:63-70`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent/src/runtime-types.ts#L63-L70).

The durable half is defined separately:

> "A `Session` is the append-only source of truth for an agent's whole
> interaction history"

Source: [`packages/core/session/README.md:5`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/README.md#L5).

Operationally, `Agent` is a live process-local handle. It holds the current
status, inbox, options, scoped plugin context, and one live `Session`. The
registry tracks only live agents, and insertion enforces
`agent.id === agent.session.id`; lookup is by that shared `SessionId`
([`packages/core/agent/README.md:9-24`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent/README.md#L9-L24)).
The durable object is the Session log. Resume loads that log, creates a fresh
unpublished Agent scope, runs setup again, and publishes a replacement live
Agent under the persisted Session ID
([`packages/core/agent/README.md:37-45`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent/README.md#L37-L45)).

There is no independent Agent record, Agent CRUD surface, or Agent-definition
version. A declarative `agents[].id` is a boot configuration label. Unless the
configuration supplies an exact `sessionId`, startup mints a fresh combined
Session ID; a stable `sessionId` can restore or create history when persistence
is present
([`packages/core/agent-loop/src/index.ts:254-310`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent-loop/src/index.ts#L254-L310),
[`packages/core/agent-loop/src/index.ts:355-380`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent-loop/src/index.ts#L355-L380)).

Conceptual model: **agent-as-live-session-bound-handle**. The Session is the
persistent identity and history; the Agent is the currently resident driver
and scoped capability surface for exactly that Session.

## Subagents

Subagents are a provider-backed runtime seam, not methods on the core Agent
interface. A parent dynamically starts work through one of several named
providers, which may create a local child, use another process, or use another
transport
([`packages/subagent/subagent/README.md:5-29`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/README.md#L5-L29)).
Provider registration is declarative composition, but each child identity is
created at runtime.

The release has two child lifecycles:

- A **one-shot** local child is an ordinary Agent and Session created for one
  delegated task. The run returns the child's shared Session ID, and the run
  holder must dispose it after the result settles. A remote provider can
  return a lifecycle ID without any local Agent or Session
  ([`packages/subagent/subagent/README.md:64-70`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/README.md#L64-L70)).
- A **continuable** child has one durable Session and at most one process-local
  Activation, meaning one residency epoch for a reconstructed Agent. Later
  messages either reach the resident Agent or cold-resume a new Activation
  from the same Session
  ([`packages/subagent/subagent/README.md:72-78`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/README.md#L72-L78)).

For local children, the parent preset composition is joined first, then the
child persona and tool filter narrow or shadow it. The child inherits the
parent provider, model, output-token cap, working directory, and durable
lineage unless explicitly overridden. It does **not** inherit the parent's
runtime tool restrictions or authority as a capability set
([`packages/subagent/subagent/README.md:35-58`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/README.md#L35-L58),
[`packages/subagent/subagent-in-process-driver/README.md:9-23`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent-in-process-driver/README.md#L9-L23)).
Delegation separately snapshots the parent's explicit sandbox override and
pins child approval policy to `never`, so unattended children cannot request
an escalation
([`packages/subagent/subagent/README.md:60-62`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/README.md#L60-L62)).

Context inheritance depends on the provider:

- `spawn` creates an empty child conversation while inheriting model and
  workspace defaults
  ([`packages/subagent/subagent-spawn-in-process/README.md:5-15`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent-spawn-in-process/README.md#L5-L15)).
- `fork` seeds only the balanced prefix ending at the parent's last completed
  turn. It excludes the in-flight turn and does not create live context
  sharing
  ([`packages/subagent/subagent-fork-in-process/README.md:5-19`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent-fork-in-process/README.md#L5-L19),
  [`packages/subagent/subagent-fork-in-process/README.md:58-61`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent-fork-in-process/README.md#L58-L61)).

Nesting is supported. Delegation depth is persisted in the child Session
header and cannot be lowered by fresh runtime options after resume. The
model-facing delegation tool defaults `maxDepth` to `3`; deployments can set
another non-negative limit or defer the limit to the provider
([`packages/subagent/subagent/src/depth.ts:11-35`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/depth.ts#L11-L35),
[`packages/subagent/tool-subagent/src/index.ts:69-99`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/tool-subagent/src/index.ts#L69-L99)).
No global fan-out limit is documented.

One-shot communication is final-result only: the parent receives the child's
final output or stop reason, not intermediate work
([`packages/subagent/subagent-spawn-in-process/README.md:39-51`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent-spawn-in-process/README.md#L39-L51)).
Continuable children instead support parent follow-ups, child-selected reports,
interrupts, and a runtime-authored settlement notice. This is an explicit
message channel, not shared conversation state
([`packages/subagent/subagent/README.md:18-27`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/README.md#L18-L27)).

## Configuration surface (what, where, why)

DeepSeek Harness distributes configuration across plugin-owned surfaces rather
than treating Agent as one definition object.

| Concern | Surface and binding | Product rationale |
| --- | --- | --- |
| Deployment capabilities | A profile in Harness home stacks bundles and patch files into a plugin tree at boot. The base bundle supplies model adapters, tools, persistence, sandbox and approval policy, settings, credentials, and telemetry. | Every product part remains replaceable and higher layers can patch lower ones. See [`docs/architecture.md:11-35`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/architecture.md#L11-L35). |
| Agent bootstrap | `agents[]` config supplies a label, optional exact or resume Session ID, provider, model, output-token cap, and fresh-session `cwd`. Configured entries start automatically. | Exact IDs support restore or resume, while fresh default IDs avoid restart collisions. See [`packages/core/agent-loop/README.md:36-56`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent-loop/README.md#L36-L56). |
| Per-agent tools and instructions | An agent preset is a directory containing `agent.cordis.yml`. It contributes scoped tools and prompt sections. A persona row can shadow the deployment persona or own the complete prompt. | Several differently composed agents can share one process without leaking registrations between sessions. See [`packages/preset/README.md:1-16`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/preset/README.md#L1-L16) and [`packages/preset/persona/README.md:5-21`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/preset/persona/README.md#L5-L21). |
| Programmatic Agent setup | `ctx.agents.create()` and `resume()` accept `AgentOptions`, Session metadata, and trusted setup code that registers scoped tools, prompt sections, variables, restrictions, and listeners before publication. | Observers never see a partially configured Agent, and setup failure rolls the whole unpublished transaction back. See [`packages/core/agent/src/index.ts:73-155`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent/src/index.ts#L73-L155). |
| Prompt and tool presentation | The deployment persona, scoped persona, ordered sections, runtime contexts, variables, and visible tool schemas assemble for each step. | Prompt text and tool presentation form one coherent model-facing request while plugins retain ownership of their facts. See [`packages/core/system-prompt/README.md:5-35`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/system-prompt/README.md#L5-L35). |
| Tool concurrency | `maxParallelToolCalls` defaults to 10 and can be changed through Settings for the next tool group. | Parallel-safe work is bounded, while `1` provides serial execution. See [`packages/core/agent-loop/README.md:36-52`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent-loop/README.md#L36-L52). |
| Child behavior | The delegation tool chooses provider, one-shot or continuable background mode, model override, persona, tool allow or deny filter, output schema, and depth cap. | The provider contract advertises capabilities so unsupported restrictions fail before a child is published. See [`packages/subagent/subagent/README.md:29-48`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/README.md#L29-L48). |

The base `AgentOptions` type contains only provider, model, and per-request
output-token cap; persona belongs to system-prompt sections
([`packages/core/agent/src/runtime-types.ts:23-31`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent/src/runtime-types.ts#L23-L31)).
Memory, credentials, triggers, schedules, sandboxing, and filesystem policy are
therefore not fields on an Agent record. Where composed, they are separate
plugin capabilities or durable Session events.

## Binding time

- **Boot:** profiles, bundles, patches, and the declarative `agents[]` roster
  compose the process. The roster is intentionally boot-only. User patch HMR
  can recompose the global plugin tree, but it does not turn the roster into a
  live Agent-definition store
  ([`packages/boot/app-boot/README.md:36-45`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/boot/app-boot/README.md#L36-L45),
  [`packages/core/agent-loop/src/index.ts:239-271`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent-loop/src/index.ts#L239-L271)).
- **Create or resume:** the exact shared ID, Session metadata, Agent options,
  setup contributions, and preset generation bind before publication. A preset
  file edit starts a new generation for later sessions; existing agents keep
  their joined generation
  ([`packages/preset/agent-presets/README.md:29-45`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/preset/agent-presets/README.md#L29-L45)).
- **Blank-session exception:** a produced-nothing Agent can switch presets. The
  product forbids switching after conversation output because logged tool calls
  could refer to a capability the new composition cannot provide
  ([`packages/preset/agent-presets/README.md:47-51`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/preset/agent-presets/README.md#L47-L51)).
- **Each step:** the loop assembles the current system prompt, visible tool
  schemas, dynamic contexts, and variables. It logs a request header containing
  the exact request envelope, so later reconstruction can see what the model
  actually received
  ([`packages/core/agent-loop/README.md:85-99`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent-loop/README.md#L85-L99),
  [`packages/core/session/README.md:61-73`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/README.md#L61-L73)).
- **Resume:** persisted history and identity return, but the caller supplies
  fresh Agent options and setup. A Session can therefore retain continuity
  while its newly resident Agent uses different route or scoped composition.
  There is request-level evidence of the resulting behavior, but no versioned
  Agent definition pinned by the Session
  ([`packages/core/agent/src/index.ts:135-155`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent/src/index.ts#L135-L155)).

Compatibility is explicitly pre-release. The repository warns that breaking
changes will occur, directs maintainers to prefer a correct foundation over
compatibility shims, and says old on-disk formats are rejected
([`README.md:9-11`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/README.md#L9-L11),
[`AGENTS.md:5-7`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/AGENTS.md#L5-L7)).
`SESSION_FORMAT_VERSION` remains `0`; current readers refuse unsupported older
or newer formats rather than promise a migration path
([`packages/core/session/README.md:139-144`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/README.md#L139-L144)).

## Relationships between nouns

| Relationship | Operational cardinality and ownership |
| --- | --- |
| Agent to Session | Every live Agent has exactly one Session, shares its exact `SessionId`, and drives only that Session. A persisted Session can have zero live Agents; resume reconstructs at most one live Agent under the same ID. |
| Config label to Agent | One boot label normally produces one freshly identified live Agent per startup. An explicit `sessionId` or `resumeSessionId` makes Session identity stable; the label itself is not the durable identity. |
| Session to turn to step | A Session contains an append-only event log. One turn contains zero or more steps; one step is one model request plus the tools it invokes ([`docs/architecture.md:63-96`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/docs/architecture.md#L63-L96)). |
| Agent to preset | One Agent scope may join one preset generation; one standing preset mount can serve many isolated Agent and Session pairs. |
| Parent to local child | One parent can dynamically create many child Sessions. The child's immutable header records `parentSession`; live registry ownership is tracked separately from durable lineage ([`packages/core/agent/README.md:19-24`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent/README.md#L19-L24)). |
| Continuable child to Activation | One durable child Session has zero or one live Activation. Each Activation owns one reconstructed Agent residency epoch. |
| Agent to workspace or sandbox | `cwd` is immutable Session creation metadata. It does not imply a sandbox; filesystem, subprocess, sandbox, approval, and credential capabilities come from the composed plugin tree. |
| Agent to tool | Tools are scoped registry contributions resolved during prompt assembly and guarded again at execution. They are not embedded in the Agent object. |

The exact Agent-to-Session relationship is the unusual part of this model.
It is not one definition serving many Sessions, and it is not a Session owning
several runs of an Agent object. The live Agent and durable Session are two
runtime views of one identity. The Agent can disappear and later be recreated;
the Session is the continuity boundary.

## Lifecycle

Creation and resume are rollback-covered transactions: construct a private
Session, Agent, and scoped context; await setup; enter both registries; announce
Session then Agent creation; emit session start; and only then start the driver
([`packages/core/agent-loop/README.md:9-26`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent-loop/README.md#L9-L26)).
Configured agents are owned by the loop fiber. Programmatic callers receive an
`AgentHandle`; its disposer is the consumer capability that stops and drains
the loop, unregisters the Agent, removes the live Session from the store, and
unwinds scoped registrations. Provider unload is an independent structural
teardown edge
([`packages/core/agent/src/index.ts:158-175`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent/src/index.ts#L158-L175)).

The observable live states are only `idle` and `running`; disposal is registry
removal, not a third status. `cancel()` aborts current activity and normally
clears queued work. The core API does not define Agent pause, hibernate,
archive, or delete states
([`packages/core/agent/src/runtime-types.ts:43-50`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent/src/runtime-types.ts#L43-L50)).
Session persistence is supplied by plugins that mirror the event log and later
reconstruct it. Model history, forks, resume, transcripts, and recovery derive
from that log
([`packages/core/session/README.md:89-93`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/session/README.md#L89-L93)).

Parent teardown has two distinct outcomes. During orderly process-local
shutdown, the subagent runtime closes new admission, cancels continuable
descendants top-down, releases them child-first, and attempts a final flush
([`packages/subagent/subagent/src/index.ts:294-325`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/index.ts#L294-L325),
[`packages/subagent/subagent/src/continuation.ts:746-841`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/continuation.ts#L746-L841),
[`packages/subagent/subagent/src/continuation.ts:1332-1394`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/src/continuation.ts#L1332-L1394)).
That kills process-local child activity, not the durable child Session. A hard
process crash runs none of this teardown; the child Session remains persisted
with its parent lineage and no durable parent-death disposition.

The shipped loop is Harness-owned code running in the user's process. It is
the only package containing concrete loop logic, but the interface is designed
to be replaced by another Agent implementation. Plugins add behavior at live
events and capability seams rather than patching the loop
([`packages/core/agent-loop/README.md:5-7`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent-loop/README.md#L5-L7),
[`packages/core/agent/README.md:77-81`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/core/agent/README.md#L77-L81)).

## What makes it "an agent" here (our inference)

Our inference: in DeepSeek Harness, an agent is the live, scoped capability
and control handle that runs an autonomous model-and-tool loop for exactly one
durable Session. It differs from a plain LLM call because it owns an inbox,
multi-step turn lifecycle, tool execution, cancellation, scoped plugin world,
and event-sourced history.

The important architecture lesson is that durable Agent identity does not
require a separate Agent-definition record. DeepSeek Harness aliases Agent
identity to Session identity, then recreates the live behavior-bearing handle
around persisted history. This makes continuity explicit, but it does not by
itself pin a versioned behavior definition across resume.

## Open questions

- There is no Agent-definition version or durable snapshot tying a resumed
  Session to the same Agent options, preset generation, global plugin tree, or
  setup code. Request headers preserve what each step observed, but the release
  does not state a reproducible replay contract for future turns after upgrade.
- Session format `0` deliberately has no broad compatibility promise. The
  project does not publish a migration policy for persisted sessions created by
  this release candidate.
- Continuable child residency is process-local and has no multi-process lease;
  the source documents this as a current limitation
  ([`packages/subagent/subagent/README.md:146-155`](https://github.com/deepseek-ai/deepseek-harness/blob/141eb6fef83422698aef7a981029e843e8161534/packages/subagent/subagent/README.md#L146-L155)).
- Remote one-shot providers may have no local child Session, so their runs are
  absent from trace-backed child enumeration. No cross-provider durable child
  identity contract is documented.
- No global subagent fan-out limit or cross-process orphan policy is specified.
- Agent archival and persisted Session deletion are outside the core lifecycle
  described by the cited sources.
