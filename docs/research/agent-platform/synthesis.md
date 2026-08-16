# Synthesis: what the industry means by "agent"

Part of Agent Definition Research.
Every product dossier, one question: what does the noun "agent"
operationally refer to? Purpose: extract the invariant core our own agent
service must model, and the axes where products deliberately diverge.
This synthesis is frozen as decision-time input: where a conclusion here
differs from an accepted record in the [ADR index](../../adr/index.md), the
ADR is authoritative.

> [IronClaw](./products/ironclaw.md) was researched and added after this
> synthesis was first frozen. Its evidence revised Convergence #2 (the trio),
> #3 (the definition's content), #4 (pinning), and #7 (tool restriction),
> Divergences A, B, C, and D, and Design decisions 1 and 4; those revisions
> are marked inline. It is the first product in the corpus with *no* agent
> object of any kind, so it is the sharpest available test of the working
> definition below, and the definition did not survive intact.

> [LangChain Deep Agents](./products/deep-agents.md) and
> [LangSmith Managed Deep Agents](./products/managed-deep-agents.md) were
> researched on 2026-08-16. Their evidence revises Convergence #4,
> Divergences C, D, and E, and the conceptual and comparison tables. The
> revisions are marked inline.

## Convergence

**1. The behavioral definition is settled.** Every product that states one
lands on the same sentence: an agent is an LLM using tools in a loop, where
the *model* decides control flow. Vercel says it verbatim ("LLMs that use
tools in a loop"); LangGraph defines agents against workflows ("workflows
have predetermined code paths... agents define their own"); Cloudflare
("autonomously execute tasks by making decisions about tool usage and
process flow"); ADK ("self-contained execution unit designed to act
autonomously"); Claude Code's glossary makes the loop itself the definition
("gather context, take action, verify results, and repeat"). Nobody defines
the agent as the model, and nobody disputes that the loop, tool-use, and
autonomy triad is what separates an agent from an LLM call.

**2. Every platform decomposes into the same trio.** Wherever "agent" is a
durable thing, three resources appear, always separated:
*definition* (config/identity), *execution* (session/run/thread/task), and
*memory* (attachable, never embedded).
[OpenComputer](./products/opencomputer.md):
agent / session / no-cross-session-memory-by-design.
[Managed
Agents](./products/claude-managed-agents.md): agent / session / memory stores.
[LangGraph](./products/langgraph-platform.md):
assistant / thread+run / store.
[AgentCore](./products/bedrock-agentcore.md):
runtime / session / memory resource.
[Devin](./products/devin.md): org config /
session / knowledge. Even the personal daemons follow it:
[Hermes](./products/hermes-agent.md) and
[OpenClaw](./products/openclaw.md) keep identity
(files), sessions (transcripts), and memory (markdown) as separate
artifacts. **This trio is the invariant core.**

*Revised after IronClaw.* The trio survives, but IronClaw shows the first
member can be absent as a *stored resource* and still be present as a
concept. There is no agent record anywhere: `AgentId` is a validated scope
string and nothing else, verified by the absence of any `AgentRecord`,
`AgentDefinition`, or agent registry in the repository. Identity lives in
markdown (`BOOTSTRAP.md`, `AGENTS.md`, `SOUL.md`, `IDENTITY.md`, `SYSTEM.md`,
`MEMORY.md`, `TOOLS.md`, `HEARTBEAT.md`, `context/*`) read per turn, execution
splits into two persisted boundaries (transcript threads and turn runs), and
memory is a file. So the correct statement of the convergence is that the
three *roles* are always separated, not that all three are always resources.
IronClaw makes the definition role a *coordinate plus a resolution*: the
`ThreadScope` tuple names who, and a `ResolvedRunProfile` captured on the run
records what was in effect.

**3. The definition's content converges.** Wherever the agent is
declarable, the same fields recur: instructions/prompt + model + tools +
limits, with skills, credentials, and delegation roster as the common
extensions. OpenComputer ("name, prompt, model, runtime, skills"), Managed
Agents ("model, system prompt, tools, MCP servers, skills"), Claude Code
frontmatter, OpenAI's constructor, CrewAI's triad + llm + tools, ADK's
LlmAgent, and eve's `agent/` directory: same shape, different serialization.

*Revised after IronClaw.* The field set converges even where the *agent* is
not declarable, which is stronger evidence for the convergence than another
agreeing agent record would be. IronClaw has no agent config, yet its
`ResolvedRunProfile` carries exactly this content one level down: a
`model_profile_id`, a `capability_surface_profile_id`, a `context_profile_id`,
a `loop_driver`, plus steering, cancellation, checkpoint, resource-budget, and
personal-context policies, a `runtime_constraints` block, and scheduling and
concurrency classes. The recurring fields are real; what varies is which noun
owns them. IronClaw attaches them to the *run*, so two runs under the same
`AgentId` can legitimately differ in model, capability surface, and loop
driver with nothing to reconcile.

**4. Many managed platforms pin sessions and version definitions.** Several
managed platforms freeze a versioned definition into the execution at
creation time: OpenComputer ("freezes the agent's active revision...
pinned for its whole life"), Managed Agents (optimistically-locked
`version`, per-session overrides never write back), LangGraph (immutable
assistant versions, full-payload updates), AgentCore (immutable runtime
versions + endpoint pointers, Vertex Agent Engine's revision traffic
splits). Vercel's Workflows reach the same end by pinning runs to
deployments. The sanctioned exceptions are deliberate: credential rotation
flows into live sessions (OpenComputer), and
[Claude
Code](./products/claude-code-agent-sdk.md) binds per *invocation* (live file reload) rather than per session,
so the file-based products trade pinning for git.

*Revised after IronClaw*, which supplies the purest form of the pin and the
reason it matters. There is no definition to version, so IronClaw pins the
*resolution* instead: a `ResolvedRunProfile` is computed once at run admission
and captured on the run, carrying a `profile_id` and `profile_version`, a
`resolution_fingerprint`, and a `provenance` record of what was consulted. The
stated motivation is recovery correctness, not auditability: a resumed or
recovered run must replay under the same `loop_driver` and
`checkpoint_schema_id` it started under, so re-resolving at resume time is a
correctness bug rather than a convenience. That is the argument for pinning
that the platforms above imply and none states this plainly. It also
generalizes the convergence: what gets frozen into an execution is not
necessarily a version number, it is whatever makes the execution replayable.

*Revised after Managed Deep Agents.* The universal claim does not hold. MDA
binds code, model, and tools to a deployment build,
but injects instructions on every run and permits instructions and skills to
change through Context Hub without redeployment. A durable thread can
therefore observe different model-visible behavior across runs. Managed
execution does not guarantee session pinning unless the platform records and
reuses every behavior-bearing input.

**5. Delegation has one output contract.** However subagents are spawned,
the parent receives only the child's final result, never its intermediate
reasoning. Claude Code ("final message verbatim as the tool result"),
Hermes ("the parent's context only sees the delegation call and the summary
result"), Vercel (`toModelOutput` condenses "a 100,000-token exploration"),
OpenAI's agent-as-tool, AgentTool in ADK, and A2A's opaque-execution
principle ("without needing to share their internal thoughts, plans, or
tool implementations"). Fresh child context is likewise near-universal, and
CrewAI's delegation prompt states the reason plainly: "they know nothing
about the task, so share absolutely everything you know."
IronClaw agrees and adds the durability half nobody else has: a child that
finishes after the parent has stopped caring is not dropped silently, it is
recorded as a `SubagentResultTombstone` naming the disposition, so "the parent
only sees the result" does not become "the result vanishes."

**6. The `description` field is the routing protocol.** LLM-driven
delegation is steered by a natural-language description everywhere it
exists: Claude Code ("Claude uses each subagent's description to decide
when to delegate"), ADK ("primarily used by *other* LLM agents to determine
if they should route a task to this agent"), CrewAI's coworker tool text,
A2A's AgentSkill descriptions. An agent's description is not documentation;
it is its API. The one product that breaks the pattern breaks it by not
having the noun: IronClaw has no agent registry to describe, so nothing
routes by description. Its triggers route by binding into the ordinary turn
pipeline, which is a reminder that description-based routing is a consequence
of LLM-selected delegation targets, not a universal requirement.

**7. Tool restriction is the safety primitive.** Constraining which tools
an agent (especially a child) may touch is the first control every product
reaches for: Claude Code `tools`/`disallowedTools`, Hermes leaf-vs-
orchestrator roles, Managed Agents permission policies (MCP defaults
`always_ask`), and OpenClaw's inherited allow/deny lists. Sandboxes come
second; tool scoping comes first.

*Revised after IronClaw*, which pushes this from a primitive to an
architecture and states the invariant the other products leave implicit:
registration is not authority. Capability access says a registered capability
"is only a possibility. It is not authority." Skills "can provide instructions
and supporting files" but "cannot grant authority." Runtime profiles "change
backend permissiveness" but do "not bypass CapabilityHost," while
`DeploymentMode` sets the ceiling. `TrustClass` caps authority independently.
And mediation is not bypassable by trusted code: "There is no private back
door for shipped loops or first-party code," because "the loop is
intentionally not the security perimeter." Four independent narrowing surfaces
that only ever intersect, none of which can widen another, is the design the
allow/deny-list products approximate with one list. The transferable idea is
that "which tools can this agent touch" should be the *intersection* of
separately-owned ceilings rather than a single field on a definition.

## Divergence

The axes where products make opposite choices (i.e., the decisions our
service must make deliberately):

**A. What the noun points at.** A spectrum from record to being:
server-side config resource (OpenComputer, Managed Agents, LangGraph's
assistant) → file on disk (Claude Code, eve, OpenClaw persona dirs) →
in-memory code object (OpenAI, CrewAI, ADK, Vercel SDK, Jido's
struct) → running stateful actor (Cloudflare's durable object, Jido's
AgentServer) → network endpoint (A2A) → the product itself (Devin) → an
accumulating learned identity (Hermes). Two instructive extremes:
[Cloudflare](./products/cloudflare-agents.md)
*fuses* identity, state, and process into one object (no definition/
instance split at all), while
[OpenAI](./products/openai-agents-sdk.md)
went the other way historically: it *had* the versioned server-side agent
resource (Assistants), deprecated it, and decomposed it into config
(Prompts) + state (Conversations) + loop (SDK).

*Revised after IronClaw*, which adds a new low end to the spectrum: **nothing
at all**. `AgentId` is declared alongside `TenantId` and `UserId` by the same
`string_id!` macro with the same scope-id validator, and that is the whole of
it. The agent is a coordinate in a scope tuple that gets projected into a
storage path and used as an authorization axis. Every property one would
expect on an agent record lives somewhere else: persona in markdown files read
per turn, runtime shape in the `ResolvedRunProfile` on the run, authority in
the deployment mode and trust class. Placing this next to Cloudflare's fusion
is the useful contrast: Cloudflare collapses definition, state, and process
into *one* object, IronClaw dissolves the definition into *none*, and both
work, which tells us the definition record is a modeling convenience rather
than a necessity. Its cost is also visible: with no record there is no place
to enumerate agents, no natural home for a description, and no per-agent
default anything, which is a real product gap and not just a purity choice.

**B. Who owns the loop.** Three positions: platform-managed loop
(OpenComputer's runtimes, Managed Agents, Devin, and the AgentCore harness
where "Who owns the loop: AWS"), customer loop behind an infrastructure
contract (AgentCore Runtime, "the orchestration loop is yours";
OpenComputer custom runtimes' `POST /turn`), and library-in-your-process
(OpenAI, CrewAI, ADK, Jido, Vercel SDK, Claude Code SDK). The AgentCore
family spans two of these on one substrate: the Runtime path is a customer
loop, while the harness is an AWS-owned loop over the same microVM, version,
and endpoint machinery. The meeting
point is the **turn contract**: OpenComputer's `POST /turn`, AgentCore's
`POST /invocations` + `GET /ping`, LangGraph's graph-nodes-as-code. The
industry has effectively standardized the *shell* (identity, sessions,
durability, isolation) without standardizing the *brain*.

*Revised after IronClaw*, which names the shell and draws it as a hard
boundary rather than an API surface. Its four layers are products (UX),
userland loops (agent behavior), a kernel boundary (authority, recovery,
side-effect mediation), and substrates (durable primitives), with loops
explicitly *demoted*: "the loop is intentionally not the security perimeter,"
and no private back door for first-party loops. That is a fourth position on
this axis, distinct from all three above: the loop is customer-replaceable
*and* untrusted, selected by a `loop_driver` on the run profile rather than
supplied over a network contract. It vindicates the shell/brain split by
making it an enforcement boundary instead of an integration point, and it is
the strongest available argument that the shell must mediate side effects
rather than merely host the loop.

**C. Binding time.** Definition or deployment freeze at session or run
creation (OpenComputer, Managed Agents, LangGraph, AgentCore, Vertex, and
Vercel Workflows), live-reload per invocation (Claude Code),
everything-at-runtime (Cloudflare), per-*step* rebinding as a designed
feature (Vercel's `prepareStep` can swap model/tools/instructions mid-loop),
kickoff-time
string interpolation (CrewAI). Hermes adds a constraint nobody else
surfaces: prompt-cache economics as the reason mid-run mutation must be
rare ("per-conversation prompt caching is sacred").
*Revised after IronClaw*, which occupies both ends at once and is coherent
about why. Persona is late-bound in the extreme, since the markdown identity
files are read per turn with no version pinning, exactly Claude Code's trade
of pinning for git. But everything that affects *replay* is bound once at
admission and frozen on the run: loop driver, checkpoint schema id and
version, model profile, capability surface, budgets. The line IronClaw draws
is the useful one to steal, and it is neither per-session nor per-invocation:
bind text late, bind mechanism early. Anything a recovery path must agree with
its original run about cannot be re-resolved; anything the model merely reads
can be.

Managed Deep Agents adds a third split within one product: code, model, and
tools bind at build and deploy time; instructions and skills bind again on
each run; thread state persists across those runs. Its useful warning is that
"deployed definition" and "behavior observed by this session" are not the
same fact when control-plane content remains live.

**D. Subagents, the least settled axis.** Declared roster with depth-1
cap (Managed Agents: 20 agents/25 threads; Hermes default; OpenClaw default,
with hierarchy frameworks explicitly refused in VISION),
deep nesting (Claude Code: depth 5, background by default), handoffs
that *transfer* the conversation rather than spawn (OpenAI, LangGraph's
Command), peer delegation by role (CrewAI), protocol-level peers with no
hierarchy at all (A2A, where the *task* is the delegated unit), dynamic
process spawning with orphan policies (Jido, the most nuanced lifetime
coupling: logical parent refs decoupled from supervision, `on_parent_death`
per child). Overlaid on all of it, Cognition's production verdict
([Devin](./products/devin.md)): parallel agents
work when they "contribute intelligence rather than actions": **fan out
reads, single-thread writes**, and fresh-context verifiers *beat*
shared-context ones for review.

*Revised after IronClaw*, which is the most conservative position in the
corpus and the only one that treats subagents as an authority problem before a
topology problem. Children are ordinary child runs with lineage on the run
record, they start with **empty grant sets** rather than inherited ones, the
tree is bounded by an atomic descendant reservation taken before any child is
queued, and in the shipped profiles the spawn capability is deny-filtered off
entirely. So the answer to "how deep can delegation go" is currently "it does
not," with the machinery built to turn it on safely later. Read against
Cognition's verdict this is the same conclusion reached from the other
direction: Devin learned empirically that parallel children must not act,
IronClaw arranges structurally that a child *cannot* act until authority is
explicitly granted. Two products, one from production experience and one from
first principles, both landing on children-are-readers-by-default is the
strongest signal on this otherwise unsettled axis.

Deep Agents makes two child lifecycles explicit under the same `subagent`
noun. A synchronous child is an isolated, blocking, stateless nested
invocation that returns one result. An asynchronous child is another graph or
assistant launched into its own durable thread and run, with update and
cancel operations. The topology label alone therefore cannot determine
whether a child needs independent identity, state, authorization, and
lifecycle.

**E. Session semantics.** Session-as-task-run (OpenComputer, Managed
Agents, Devin, LangGraph runs) vs session-as-conversation-lane (OpenClaw's
routing-scoped lanes, Hermes' session keys, Cloudflare instances that may
*be* a room). Who names it also splits: platform-minted IDs vs
caller-supplied keys (AgentCore's client-named `runtimeSessionId`,
OpenComputer's get-or-create `key`, OpenClaw's deterministic routing keys).
IronClaw refuses the choice by splitting the noun: a `SessionThread` is the
conversation lane (durably sequenced messages and summaries under a scope),
and a `turn_run` is the task run (lifecycle, locks, checkpoints, admission
reservations), persisted separately with a redaction boundary between them so
lifecycle records hold "metadata and references only." Both halves of the
divergence exist, and neither is asked to do the other's job. Inbound naming
is idempotency-keyed rather than either minted or caller-named: a SHA-256 over
`(scope, source_binding_id, external_event_id)`.

Deep Agents and Managed Deep Agents sharpen the LangGraph side of this
distinction. Their durable continuity boundary is the `thread`; a `run` is
one invocation inside it. MDA documentation sometimes calls the thread a
session, and thread-scoped sandboxes are reused across runs. Mapping the run
to a session would discard the state and environment continuity that the
product actually preserves.

**F. Identity scope.** Everyone has intra-org identity; only
[A2A](./products/adk-a2a.md) defines cross-org
identity: the AgentCard (name, skills, interfaces, security schemes, JWS
signatures, well-known URI). It is the only serious interoperable
definition, and Vertex + LangGraph + AgentCore + CrewAI all already carry
A2A hooks. IronClaw is at the far opposite end and deliberately so: its agent
identity is an internal scope axis in a `ThreadScope` tuple
(`tenant_id`, `agent_id`, optional `project_id`, `owner_user_id`,
`mission_id`), meaningful only inside the deployment and used for storage
placement and authorization rather than for discovery. With no agent record
there is nothing to project into an AgentCard, which makes the cost of the
no-record design concrete: cross-org identity would have to be synthesized
from the scope plus a run profile rather than published from a definition.

## Conceptual models in play

| Model | Exemplars |
| --- | --- |
| agent-as-config (versioned record) | OpenComputer, Managed Agents, LangGraph assistant, AgentCore harness |
| agent-as-file (git-versioned persona) | Claude Code, Vercel eve, OpenClaw workspace |
| agent-as-code-object | OpenAI Agents SDK, CrewAI, ADK, Vercel AI SDK, Jido struct |
| agent-as-process/actor | Jido AgentServer, Cloudflare durable object |
| agent-as-deployed-service | Bedrock AgentCore, Vertex Agent Engine |
| agent-as-network-endpoint | A2A AgentCard |
| agent-as-role/persona | CrewAI (role/goal/backstory), OpenClaw SOUL.md |
| agent-as-teammate/product | Devin |
| agent-as-learning-identity | Hermes (memory + self-authored skills as the definition) |
| agent-as-interface (anything that runs the loop) | Vercel AI SDK 6, Claude Code harness framing |
| agent-as-compiled-harness-graph | LangChain Deep Agents |
| agent-as-code-first-definition compiled into a managed assistant and deployment | LangSmith Managed Deep Agents |
| agent-as-scope-coordinate (a validated axis in a scope tuple; no stored object, persona in files, runtime shape resolved onto each run) | IronClaw |

These are not mutually exclusive; most products stack two or three.
IronClaw stacks agent-as-scope-coordinate with agent-as-file, which is what
makes it legible: the scope answers "whose," the files answer "who."

## Comparison table

| Product | The `agent` noun is a... | Subagents | Config binding time | Lifecycle owner | Agent : session |
| --- | --- | --- | --- | --- | --- |
| OpenComputer | versioned config record | Flue-only, untested profile; fan out via sessions | session freezes active revision | platform (custom runtime = BYO turn) | 1:N, session pins revision |
| Claude Managed Agents | versioned identity+config resource | declared roster, depth 1, max 20/25 threads | session pins version; tools/MCP mutable when idle | platform (self-hosted = tool exec only) | 1:N, session = running instance |
| Jido | immutable struct + module (process separate) | dynamic spawn directive, logical refs, orphan policy | compile-time macro; state via cmd/2 | customer BEAM app | no session noun |
| Claude Code / Agent SDK | markdown file → fresh context window | richest: 5 scopes, depth 5, fork, background | live reload per invocation; no versions | customer process (harness/SDK) | subagent ⊂ session |
| OpenAI Agents SDK | code object (dataclass) | handoffs (transfer) + agent-as-tool (return) | construction + per-run overrides; no versions | customer process | sessions independent of agents |
| Bedrock AgentCore | ARN'd deployed container service | none, inside the container; A2A between peers | immutable versions + endpoint pointers | customer loop, AWS shell | 1:N, session = microVM |
| LangGraph Platform | assistant = versioned config over a graph | subgraphs + Command handoffs, declared in code | deploy (graph) / versioned (assistant) / per-run | platform executes customer graph | assistant N:M threads |
| Google ADK + A2A | code object in a tree / endpoint + card | first-class sub_agents tree; A2A = opaque peers | construction / discovery-time (card) | customer (ADK) / managed (Vertex) | runner binds agent↔sessions; task = unit of work |
| Cloudflare Agents | durable object (identity+state+process fused) | child DOs, RPC, kill vs delete distinct | everything at runtime; deploys restart | customer handlers, platform persistence | instance may *be* the session |
| CrewAI | role persona (pydantic) | peer delegation + manager hierarchy | kickoff interpolation; no versions | customer process (in-lib loop) | run = kickoff; no session |
| Devin | the product itself (teammate) | managed Devins, coordinator + children, ACU caps | session creation; config = org onboarding | Cognition entirely | 1 agent : N sessions (VM each) |
| Vercel (SDK + platform) | interface over the loop; eve = file dir | subagent-as-tool, toModelOutput condensing | construction + per-step rebinding; Workflow pins deploys | customer code; platform durability opt-in | no session (SDK); eve adds one |
| Hermes Agent | learning identity (files) + per-session process | delegate_task, leaf/orchestrator, depth 1, cost-capped | session assembly; cache-preservation constrains mid-run | user-run daemon | 1 identity : N session lanes |
| OpenClaw | resident persona (workspace dir) | sessions_spawn, depth 1, agent-to-agent allowlisted | hot reload; per-message routing | user-run gateway daemon | 1:N routing-scoped lanes |
| Netclaw (added post-synthesis) | daemon with file-shaped soul; event-sourced actor sessions | spawn_agent → ephemeral child actors, depth 1 (recursive spawn denied), fail-closed audience inheritance | daemon-start validation; validate-before-restart reload with session drain; identity re-read per session actor | user-run daemon (systemd) | 1:N channel+thread-keyed persistent actors |
| kagent (added post-synthesis) | namespaced K8s custom resource reconciled into an A2A service | agent-as-tool by CRD reference; DAG capped at depth 10; fresh child session, identity-only inheritance | reconcile-time resolution into a config Secret; rebinding = pod roll | platform deploys; in-pod ADK runtime owns the loop | 1:N DB sessions; delegation mints child sessions |
| AgentCore harness (added post-synthesis) | versioned config record over an AWS-owned loop | no subagent noun; agent-as-tool via Gateway; compose above via Step Functions | auto-versioned config; per-call overrides | AWS owns the loop (managed harness on managed Runtime) | 1:N, session = microVM |
| [IronClaw](./products/ironclaw.md) (added post-synthesis) | scope coordinate with no stored object; persona in markdown, runtime shape in a `ResolvedRunProfile` on the run | child runs, lineage on the run, empty grant sets, atomic descendant reservation, deny-filtered off in shipped profiles | persona per turn (file read); mechanism resolved once at admission and frozen on the run | userland loop above a kernel boundary owning authority and recovery; loop is not the security perimeter | 1:N threads under the scope; thread (transcript) and turn run (lifecycle) are separate resources |
| [LangChain Deep Agents](./products/deep-agents.md) (added post-synthesis) | compiled LangGraph harness graph; no durable Agent resource | sync child = stateless nested invocation; async child = independent thread and run | construction plus per-run context; thread state via checkpointer; no definition version contract | customer process or surrounding deployment | graph serves N threads; thread is the session boundary, run is one invocation |
| [LangSmith Managed Deep Agents](./products/managed-deep-agents.md) (added post-synthesis) | code-first pre-runtime definition compiled into an assistant and deployment | underlying Deep Agents sync and async models; no MDA-specific child resource | code/model/tools at build; instructions/skills on every run; state on thread | LangSmith managed harness and runtime | one assistant serves N threads; session maps to thread, with N runs |

## Working definition

Derived from the evidence, for our service:

> **An agent is a named, versioned declaration of persona and capability
> (instructions, model, tools, limits, credentials, and a delegation
> roster) that a runtime instantiates into pinned, resumable executions
> (sessions), with memory and environment attached as separate resources.**
> The agent-ness (loop, autonomy) is a property of the runtime executing
> the declaration, not of the record itself.

That sentence is the research starting point, not the final ownership map:
the accepted data-ownership decision keeps behavior and dependency
declarations in the revision while assigning limits, credential bindings,
work contracts, resolved session context, and observations to their owning
resources.

*Revised after IronClaw.* The working definition assumes its own conclusion in
one place: "a named, versioned declaration" presumes the declaration is a
stored resource. IronClaw is a working system where it is not, so the honest
generalization is that an agent is a **named scope plus a resolved
configuration**, and whether that configuration is a versioned record, a set
of files, or a resolution captured per run is a product decision. Our ADRs
already choose the versioned record, and the evidence still supports that
choice for a multi-tenant platform that must enumerate, describe, and share
agents. What IronClaw changes is the *justification*: the record earns its
place by giving us discovery, description-based routing, and per-agent
defaults, not by being the only way to make executions replayable. Replay
needs a captured resolution on the execution, which we should have whether or
not the definition is versioned.

Design decisions the evidence forces, with the industry's answer where one
exists:

1. **Model the trio as three first-class resources**: AgentDefinition
   (versioned), Session (pins a definition version at create), Memory
   (attachable N:M). Do not embed memory or environment in the definition;
   nobody who scaled did. *Revised after IronClaw*: pinning a definition
   version is necessary but not sufficient. Also capture the *resolved*
   runtime shape on the execution (loop/driver identity, checkpoint schema
   version, model and capability-surface selections, budgets, and a
   fingerprint of what was consulted), because a recovery path that
   re-resolves can legally land on a different driver or checkpoint schema
   than the run it is recovering. A version pointer alone does not prevent
   that when resolution depends on anything outside the definition.
2. **Version linearly and immutably; our Session plans pin every
   behavior-bearing input.** Allow per-session overrides that never write back
   (Managed Agents), staging/rollback
   (OpenComputer, LangGraph), and exactly one live-mutation exception:
   credential rotation.
3. **Own the shell, open the brain.** Offer a managed loop *and* a
   bring-your-own-loop turn contract (`POST /turn`-shaped): the point
   where OpenComputer, AgentCore, and Managed Agents self-hosted all
   converged independently.
4. **Subagents: declared roster, depth 1 by default, results-only return,
   restricted-tool inheritance.** Depth and fan-out are cost controls
   (Hermes) as much as safety ones. Enforce Cognition's rule structurally
   if possible: parallel children for reads/analysis; single writer.
   *Revised after IronClaw* on two points. First, invert the inheritance
   default: children should start with an **empty** grant set that the parent
   must explicitly narrow *into*, rather than inheriting the parent's tools
   minus a deny list, because a deny list has to anticipate every dangerous
   capability while an allow list only has to name the needed ones. Second,
   bound the tree by **reserving descendant slots atomically before queueing
   any child**, not by checking a depth counter at spawn time, which is the
   only form of the limit that holds under concurrent fan-out. IronClaw also
   demonstrates the shippable intermediate state worth copying: build the
   lineage, reservation, and tombstone machinery, then keep the spawn
   capability denied by default until the authority story is finished.
5. **Make `description` a first-class, prompt-visible field**: it is the
   delegation routing contract, not metadata.
6. **Name sessions with caller-supplied idempotency keys** (get-or-create),
   the pattern AgentCore, OpenComputer, and OpenClaw share: it makes the
   session a routing lane, not just a run.
7. **Export identity as an A2A AgentCard.** It is the only cross-vendor
   agent identity that exists; LangGraph, Vertex, and AgentCore already
   emit or embed it.
8. **State the trust boundary explicitly** (OpenClaw's lesson): one
   definition namespace per trusted operator/org; adversarial isolation
   lives at the process/sandbox boundary, not inside the agent model.
9. **Treat prompt-cache economics as a design input** (Hermes's lesson):
   whatever binds mid-run must not invalidate the cached prefix; prefer
   session-splitting (compression → new pinned session) over in-place
   mutation.

The one-line reading of the whole study: the industry agrees on the
*sentence* (an LLM using tools in a loop) and on the *trio*
(definition / execution / memory); everything else (where the definition
lives, who runs the loop, how deep delegation goes) is a product decision,
and the most successful designs are the ones that made those decisions
explicit rather than inheriting them.

Revised after IronClaw: the trio is a set of *roles*, not necessarily a set of
resources, and IronClaw is the one that proves it by shipping without the
first member. What that reframing buys us is a sharper test for
our own design. Every property we are tempted to put on the agent definition
should have to answer why it belongs to the agent rather than to the scope
(authorization), the files (persona), or the run (resolved mechanism). The
properties that survive that test are the ones a definition record genuinely
owns; the rest are there because a record was the first place we had to put
them.
