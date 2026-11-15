---
title: "Standard Agents: what 'agent' means"
source_urls:
  - https://github.com/standardagents/spec/tree/78fa54c14d7125f481f6f05538cce5abb5b7a419/content/0.2.0
  - https://standardagentspec.org/0.2.0/specification/agents
retrieved: 2026-11-15
status: done
---

# Standard Agents: what "agent" means

Part of the [Agent platform research corpus](../index.md).
Produced using the questions and evidence rules in
[RESEARCH_PROMPT](../RESEARCH_PROMPT.md).

This is a later addition to the frozen original study. It does not revise its
[synthesis](../synthesis.md) or convert a proposal into an accepted decision.
The [contract-impact analysis](../contract-impact-standard-agents.md) compares
these findings with the inspected platform checkout. The
[source ledger](../sources/standard-agents.md) records pinned primary-source
links, exact excerpts, file digests, and the evidence boundary.

## The `agent` noun (primary-source quotes)

### Scope and version

The inspected specification is **0.2.0**, at repository commit
[`78fa54c14d7125f481f6f05538cce5abb5b7a419`](https://github.com/standardagents/spec/commit/78fa54c14d7125f481f6f05538cce5abb5b7a419),
retrieved 2026-11-15. The repository contains specification prose and its Astro
publishing site. Its root package is private site tooling, rather than the
implementation of the `@standardagents/spec` imports shown in examples.

The upstream README calls 0.2.0 current. Both 0.1.0 and 0.2.0 remain in the
tree. Version 0.1.0 describes `ai_human` and `dual_ai` agent types and tool-based
subagents; 0.2.0 replaces that model with explicit side backings and side-owned
relationships. Mixing the versions changes the execution contract.

### Definition

[Agents, opening paragraph](../sources/standard-agents.md#agent-definition):

> An agent is the top-level conversation orchestrator in Standard Agents.

> There is no agent "type": everything about how an agent converses, composes, and terminates is **derived from the backings** of its two sides.

An `AgentDefinition` requires `name`, `sideA`, and `sideB`. Each side must have
exactly one backing:

| Backing | Turn owner | Consequence |
| --- | --- | --- |
| `{ human: true }` | A person through a runtime-defined interface | Execution waits for inbound human input. Identity, notification, and reply UI are outside this declaration. |
| `{ prompt: 'name' }` | A model driven by a named prompt | The runtime runs the model/tool step loop and the configured stop rules. |
| `{ handler: 'name' }` | A deterministic function | One invocation handles a persisted inbound message; there is no model call or tool-choice loop for that turn. |

Both sides must be explicit. Human on side A is valid; human on both sides is
reserved and must be rejected. Handler/handler is not prohibited by this
rule. Human sides may have only their discriminator and optional label, and
cannot own subagents. Handler sides cannot carry prompt step/stop bindings.
Undefined backing references, conflicting relationship names, and malformed
session bindings are definition-time errors.

**Conceptual model: agent-as-executable-conversation-definition.** A name
identifies a definition; the persistent instance is a thread. There is no
specified self-certifying Agent identity, revision-governance aggregate, or
registry approval lifecycle in this definition.

## Subagents

The [subagent contract](../sources/standard-agents.md#persistent-children) is
structural. A prompt or handler side declares `SubagentRelationship[]`; any
agent may be its target, including one with a human participant. A relationship
is not an entry in `PromptDefinition.tools`.

| Concern | Specified behavior |
| --- | --- |
| Relationship identity | `as` defaults to the unqualified agent name. Distinct aliases may target the same definition. The alias is used in lifecycle tools and registries. It cannot occur on both sides or collide with a tool or reserved lifecycle name. |
| Ownership | Child completion, failure, and parent notifications route toward the declaring side's next turn. Sibling-side model access is absent; trusted `ThreadState` code paths remain available. This is a conversation-routing rule, not a tenant authorization grant. |
| Creation | Runtime-injected `subagent_create`, code using lifecycle APIs, or `immediate` at thread creation. Immediate spawning is recursive and precedes either side's first turn. |
| Model visibility | Prompt sides receive `subagents`, `subagent_create`, and `subagent_message` for non-hidden relationships. Lifecycle calls to undeclared or disabled relationships must fail without creating a child. |
| Optional branches | The named env flag enables a relationship only for `true`, `1`, or `yes`, case-insensitively. `hidden` removes model visibility while retaining code-driven creation and required parent hooks. |
| Limits | `maxInstances` caps simultaneous instances; unset means unbounded. No universal numeric nesting, fan-out, or total graph limit is specified. |
| Blocking | `blocking` waits for a child session to close. With `maxInstances: 1` and an existing instance, creation must message and await that instance, rather than fail on the cap. |
| Persistence | Every child is a persistent isolated thread. Closing, failing, or capping its session must not destroy or orphan it. A later message starts a new session with history intact. |
| Arguments | The receiving side A prompt's `requiredSchema` defines invocation input. Validate and retain initial arguments for the child's lifetime; restore them for both sides and included prompts on every activation. Missing required arguments produce `subagent_arguments_required`. |
| Variables | `scopedEnv` is runtime transfer data, not automatically model context. Immediate bootstrap may expose only the explicitly designated name/description hints. Missing scoped variables may use a deferred bootstrap endpoint flow. |
| Completion | Default `parentCommunication: 'implicit'` queues a silent completion/failure message. `explicit` suppresses automatic completion/failure notification and parent wake-up; code can still notify deliberately. |
| Visibility of children | The model queries a registry through `subagents`. An additional changing ambient registry system message is prohibited, explicitly to preserve provider prefix caching. |
| Files | Copy attachment bytes across thread boundaries in either direction and deliver destination-local paths. A source thread's path is insufficient. |

Children have separate message histories, filesystems, queues, and lifecycle.
Thread KV is not copied or inherited. Environment transfer is separately
specified; isolation of a child's storage is not proof that installed
definition code has a security sandbox.

A human-backed child can park as `awaiting_human`. The spec supplies that state
and its fulfillment by a message, but leaves the approver's identity and
notification surface to the runtime. Parent teardown is mentioned as an
explicit termination path; a complete durable cascade protocol is not defined.

## Configuration surface (what, where, why)

Definitions are TypeScript, conventionally beneath `agents/`, using
`defineAgent`, `definePrompt`, `defineHandler`, `defineTool`, `defineHook`,
`defineEffect`, model/provider factories, and generated name registries.
The registry is developer tooling for typed names; it does not establish
runtime authorization.

| Surface | Configuration and boundary |
| --- | --- |
| Agent | Explicit sides; title, description, icon; `primary` picker visibility; `exposeAsTool` and tool description; env defaults; fallback hooks; required parent hooks; optional `maxSessionTurns`. Non-primary agents remain addressable. |
| Prompt | Required name, tool description, prompt content, and model reference. Content is a string or text/include parts. Includes recurse with cycle rejection and include only prompt content. |
| Prompt controls | `includeChat` and `includePastTools` default false; tool choice defaults auto; `parallelToolCalls` controls the provider option; reasoning settings, recent-image threshold, schema, tools, variables, env defaults, hooks. |
| Handler | Optional description and `execute(state, message)`. Trusted code receives the full persisted message and `ThreadState`. A handler cannot be referenced as a tool, hook, or scheduled effect. |
| Tools | Function capabilities, nested prompt calls, or same-thread agent handoffs. Packed tools declare named invocation dependencies through `uses`. When a list is present, calls outside it are rejected; an empty list grants no targets. Omission leaves that particular restriction unapplied. |
| Model | Name, provider factory, provider model identifier, optional fallback names, routing preferences, prices, capabilities, provider options, and provider-embedded tools. Provider option validation is conditional on the provider exposing a schema. |
| Provider | Request/stream translation, typed errors, usage, optional model discovery, tools, and metadata. Connection config slots hold keys, destinations, region, timeouts, and headers; these are separate from inference `providerOptions`. Extensions have identities distinct from their base provider. |
| Hooks | Thread creation, message creation/update, system/context assembly, tool progress/results. Nonempty prompt hooks replace the agent's fallback hook set. Enabled direct children can contribute required hooks that execute against the parent. |
| Effects | Named scheduled work outside tool/turn execution, with retained args and optional delay. No result is returned to the model. Asynchronous enqueueing is required; inspection, cancellation, and handler idempotence are recommendations. |
| Thread state | Messages, isolated filesystem and durable KV, temporary per-execution context, arguments, env, events, parent/child access, cancellation and lifecycle. Optional runtime streams exchange live opaque bytes. |
| User state | Stable principal identity, durable values, env, serialized user endpoints and internal `call`, events, optional grants and room facilities. Authoritative roles cannot come from user-writable values. |
| Distribution | Self-contained npm packages with lazy definition registries and `__meta`; entry-agent exposure, package namespaces, required dependencies, endpoint exports, and parent-hook disclosures. |

The required-parent-hook rule is stronger than an installation convention:
installing is inert, but composing a child must activate its requirements, with
disclosure before enabling the relationship. Missing required hooks fail
composition closed. Hooks load from the child's package and receive the
parent's `ThreadState`; hidden enabled children contribute hooks too. They
apply to every prompt on either parent side, while requirements do not
propagate transitively through the child graph.

### Decision inference is a separate capability

`ThreadState.decide()` is mandatory during execution and at rest. It receives
an explicit provider factory, model, JSON-preserving context, and named
`choice`, `score`, or `boolean` questions. It must not implicitly read or mutate
conversation history, invoke tools, or start a turn.

Choice answers preserve the complete probability distribution. Scores are
probability-weighted zero-based rubric indices and may be fractional; legends
and available confidence survive translation. Boolean answers return P(true),
not a thresholded boolean. Rounding must not be silently renormalized.
Question/context limits are provider-defined. This probabilistic inference
primitive is unrelated to a deterministic event-sourcing decider.

### Credentials and public surfaces

The spec already defines an optional **write-only user credential grant**
mechanism: `user.fetch(grant, ...)` injects material on the runtime side,
checks destination origins and caller allowlists, denies an absent allowlist,
and must fail distinguishably after grant deletion. Packaged code, models and
clients cannot read grant material, including through reachable errors/logs.
Grant support is SHOULD-level; these enforcement requirements are mandatory
when it is supported. This is more than a secret display label.

Separately, the spec requires secret classification for provider credentials and
keeps credential material out of user identity records. Provider transports support
an injected `fetch`; decision-provider rules prohibit destination overrides
from redirecting inherited credentials. The optional `instanceEnv` capability
reads only instance-owned trust anchors and must be feature-detected by
consumers that require it.

It does not establish that ordinary tools, hooks, or provider factories never
receive secret values. Those are trusted definition code with broad state
access. Encryption wording also varies by surface: generic env guidance is
conditional, while immediate-child runtime transfer expressly requires
encryption at rest. Secret labels alone do not prove host-owned custody.

Custom thread endpoints default to authentication plus thread access checks.
An explicitly `public` endpoint must be invoked even for anonymous requests;
the handler receives resolved caller identity and `threadAccess` separately.
The thread owner and caller are distinct. Public endpoint authors therefore
own the application's additional authorization checks.

## Binding time

| Moment | Bound or resolved |
| --- | --- |
| Definition/load | Sides, prompt/model/tool references, relationship aliases, named dependency closure, hook requirements, schemas, package namespaces. Definition validation rejects illegal shapes. |
| Thread creation | Initial agent, owner, arguments, initial variable bindings, isolated storage, and recursive immediate children. |
| Activation/request | Active side/prompt, restored child arguments, variable interpolation, effective hooks, eligible history snapshot, provider config/transport, enabled branches and model options. |
| During thread lifetime | Messages and files can be changed; `setEnv` persists thread values and propagates to active descendants except descendant scoped-variable barriers. Agent handoff preserves the thread and history. |
| Packaging | Package identity/version, entry agents, required components and hook disclosures. This is distribution metadata, not a specified admission receipt over a complete runtime closure. |

Env precedence is **thread → agent defaults → prompt defaults → user → runtime
instance**. `envType` classifies display/redaction and defaults unknown or
non-thread sources to secret. A higher-precedence value is not automatically
an authoritative service destination or credential source.

There is no specified immutable AgentRevision/SessionExecutionPlan binding,
hot-definition-update protocol, content digest admission, or definition
rollback governance. Model fallback is recommended after primary retries, in
declared order. It is not an exact-route preservation guarantee.

## Relationships between nouns

| Relationship | Meaning |
| --- | --- |
| Agent → sides | Exactly two explicit side configurations, each with one backing. A prompt or handler side may own child relationships. |
| Definition → threads | A definition may be instantiated in persistent isolated threads. A thread's assigned agent can change by handoff. |
| Thread → sessions/executions | A thread survives session completion and can activate again. An active execution exposes `flowId`, side, step counters, abort signal and live persisted history. A session terminal boundary is distinct from stopping an execution loop. |
| Prompt → sub-prompt | A nested prompt invocation is a tool in the same thread; prompt path and parent/depth record nesting. It does not create a child thread. |
| Prompt → handoff | An exposed agent with an appropriate human side takes over the same thread, preserving history. |
| Side → child thread | A declared relationship creates an isolated instance. Blocking controls waiting, independently of persistence. |
| Thread → user | An optional owner principal exposes user state. User endpoints serialize per user; plain value accesses outside those executions do not gain that guarantee. |
| Package → definitions | Internal definitions are namespace-isolated; external callers resolve entry agents. `PackageSignature` records membership metadata, not a specified cryptographic signature verification protocol. |

The package graph is an executable dependency graph, rather than a declarative
workflow language. Handler-backed participants can nevertheless implement
deterministic coordination entirely without model work.

## Lifecycle

### Runtime and persistence

The runtime owns execution, context assembly, provider calls, tool ordering,
durable work queues, stop conditions, and parent/child communication. It must
execute one step/flow at a time per thread; cross-thread concurrency is allowed.
Prompt tool calls execute sequentially in returned order, even if the provider
was allowed to generate parallel tool calls.

For a prompt step, the runtime drains queued work, assembles context, calls
the model, persists the assistant response, executes tools sequentially,
persists results, and evaluates stop conditions. Tool errors persist as results
and must not crash the execution loop. Stop evaluation prioritizes session
stop/fail bindings, then side stop-tool, response-stop, and safety limits.
`maxSessionTurns` is optional for every agent shape, so termination discipline
is partly the author's responsibility.

### Stored history and model context

Context assembly is ordered:

1. Interpolate the system prompt.
1. Run `after_system_message`.
1. Snapshot eligible persisted history.
1. Run `filter_messages` exactly once on that snapshot.
1. Transform the result into provider messages.
1. Run `prefilter_llm_history`.
1. Dispatch the request.

Messages injected before the snapshot are eligible now; messages injected
after it are eligible next time. Injection must persist and run message
lifecycle hooks, but cannot recursively restart request assembly. Synthetic
request-context edits must not replace live `execution.messageHistory`.
`silent` changes ordinary client presentation, not persistence or model
eligibility. Reasoning actually returned by a model must be persisted and
replayed in a provider-acceptable form when included in the next context,
subject to the prompt's explicit reasoning exclusion.

### Handler delivery and errors

The handler contract says exactly-once invocation per inbound message, in
persistence order, with no concurrent handler invocation on a thread and no
automatic redelivery after a thrown handler. A throw fails and logs that turn;
the message remains persisted and the session continues. The next message
gets a new invocation. This does not specify a crash-atomic external side
effect protocol.

A blocking child awaited inside a handler holds the parent's serialized
delivery slot. If the child needs another message on that parent to finish,
the design deadlocks. Upstream recommends nonblocking relay handlers.

### Session stop, execution stop, and thread termination

`execution.stop()` stops the current execution loop without satisfying the
child session's terminal boundary. `stopSession()` ends the active session
without terminating the thread. Only explicit termination, operator action,
or parent teardown destroys the child instance's availability for new work.
Thread termination records a timestamp, aborts in-flight work, rejects new
execution/queue/send, and updates the linked parent registry.

Run status is independently persisted as idle/running/failed. A lost executor
must settle to failed with `execution_lost`; it cannot leave a permanently
running status. Human waiting appears as idle plus a reason at the run surface
and `awaiting_human` in a child registry. Retention and idle-thread archival are
runtime-defined.

### Storage, sandbox, and clients

Thread KV must survive executions, restarts, and redeploys, with isolated
per-thread keys, JSON-equivalent values, and null/undefined deletion. It is
distinct from per-execution `context`, the filesystem, env, and user values.
Storage capacities are runtime-defined; generic KV encryption is optional.

When `runCode` is exposed, its contract requires a fresh JS/TS isolate per call,
no ambient host APIs, explicit imports/globals, clone/proxy marshaling,
restricted dynamic code/import facilities, and specified result statuses.
Memory enforcement is conditional on engine accounting. Deadlines are imposed
through handle termination rather than a silently invented wall-clock timeout.
This sandbox applies to `runCode`, not automatically to installed hooks,
handlers, or tools.

The thread HTTP/WebSocket client API is a SHOULD-level capability. Implementing
it requires every specified route, bearer authentication, per-thread access,
single-use path-bound stream tickets, durable message acceptance before the
resulting execution completes, prescribed errors, stream filtering, and an
initial run-status snapshot. Deletion must make the thread and descendants
unavailable and abort/discard work before confirming. Runtime byte streams are
separately optional, host-authorized, live, and non-durable.

User scope has its own required state, endpoint, event, and serialization
contracts. Grants are recommended, with mandatory policy enforcement when
implemented; explicit room attachment admission is conditional. Thread client
API support, user endpoints, custom thread endpoints, and runtime byte streams
are distinct surfaces with different conformance levels.

## What makes it "an agent" here (our inference)

Our inference: an agent is a portable executable contract for a two-party
conversation, whose participants may be people, models, or deterministic
functions. Agency is defined through turn ownership and composition rather
than requiring a model, persistent organizational identity, or a particular
hosting backend.

For the platform, the strongest fit is a candidate concrete registered
implementation behind an admitted execution boundary. Its abstract identity,
authorization and credential-grant contracts do not implement the platform's
identity, revision, policy, custody, or admission services.
That recommendation is research, not an adopted runtime choice.

## Open questions

- **Crash boundaries for exactly-once handlers.** The spec requires ordered
  invocation and forbids automatic redelivery after a throw. It does not
  define the atomic protocol for a process dying between recording an
  invocation, performing an external effect, and persisting its outcome.
- **Parent-hook order.** Hooks defines side A then side B relationship order;
  Runtime still describes child hooks in prompt tool order. Relationships
  no longer live in prompt tools in 0.2.0. An implementation needs an
  explicit interpretation or upstream clarification.
- **Hook failure policy.** The general hook description calls failures
  non-fatal, while before-create/update sections recommend preventing writes
  on failure. The Messages introduction also groups these storage hooks with
  non-destructive request-context filters. Implementers must distinguish
  persisted mutation from request-only transformation.
- **Code termination timing.** Runtime resource enforcement uses SHOULD for
  50 ms settlement with a synchronous-loop caveat; its checklist uses MUST
  for settlement within 50 ms. The exact guarantee needs reconciliation.
- **Cancellation and generated attachments.** Open upstream issues
  [#52](https://github.com/standardagents/spec/issues/52) and
  [#46](https://github.com/standardagents/spec/issues/46), checked
  2026-11-15, ask for queued-message cancellation and sub-prompt-generated
  file attachment semantics respectively. Their existence does not imply
  acceptance of any proposed solution.
- **Admission and bounded execution.** Immutable definition closure,
  definition-update behavior, recursive immediate-graph cycles, global
  resource budgets, effect retry/dead-letter policy, and complete teardown
  recovery are not established by the inspected contracts.
- **Independent portability evidence.** The runtime directory lists Standard
  Agents Builder on Cloudflare Workers and explicitly disclaims conformance
  certification. The inspected repository's tests exercise the publishing
  site; no executable cross-runtime conformance suite was identified in its
  tracked tree. No runtime or SDK package was executed for this study.
