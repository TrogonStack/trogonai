# Synthesis: what the industry means by "agent"

Part of Agent Definition Research.
Seventeen product dossiers, one question: what does the noun "agent"
operationally refer to? Purpose: extract the invariant core our own agent
service must model, and the axes where products deliberately diverge.
This synthesis is frozen as decision-time input: where a conclusion here
differs from an accepted record in the [ADR index](../../adr/index.md), the
ADR is authoritative.

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

**3. The definition's content converges.** Wherever the agent is
declarable, the same fields recur: instructions/prompt + model + tools +
limits, with skills, credentials, and delegation roster as the common
extensions. OpenComputer ("name, prompt, model, runtime, skills"), Managed
Agents ("model, system prompt, tools, MCP servers, skills"), Claude Code
frontmatter, OpenAI's constructor, CrewAI's triad + llm + tools, ADK's
LlmAgent, and eve's `agent/` directory: same shape, different serialization.

**4. Sessions pin; definitions version.** Every managed platform freezes
the definition into the execution at creation time and versions the
definition linearly: OpenComputer ("freezes the agent's active revision...
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

**6. The `description` field is the routing protocol.** LLM-driven
delegation is steered by a natural-language description everywhere it
exists: Claude Code ("Claude uses each subagent's description to decide
when to delegate"), ADK ("primarily used by *other* LLM agents to determine
if they should route a task to this agent"), CrewAI's coworker tool text,
A2A's AgentSkill descriptions. An agent's description is not documentation;
it is its API.

**7. Tool restriction is the safety primitive.** Constraining which tools
an agent (especially a child) may touch is the first control every product
reaches for: Claude Code `tools`/`disallowedTools`, Hermes leaf-vs-
orchestrator roles, Managed Agents permission policies (MCP defaults
`always_ask`), and OpenClaw's inherited allow/deny lists. Sandboxes come
second; tool scoping comes first.

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

**C. Binding time.** Freeze-at-session (all managed platforms),
live-reload per invocation (Claude Code), everything-at-runtime
(Cloudflare), per-*step* rebinding as a designed feature (Vercel's
`prepareStep` can swap model/tools/instructions mid-loop), kickoff-time
string interpolation (CrewAI). Hermes adds a constraint nobody else
surfaces: prompt-cache economics as the reason mid-run mutation must be
rare ("per-conversation prompt caching is sacred").

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

**E. Session semantics.** Session-as-task-run (OpenComputer, Managed
Agents, Devin, LangGraph runs) vs session-as-conversation-lane (OpenClaw's
routing-scoped lanes, Hermes' session keys, Cloudflare instances that may
*be* a room). Who names it also splits: platform-minted IDs vs
caller-supplied keys (AgentCore's client-named `runtimeSessionId`,
OpenComputer's get-or-create `key`, OpenClaw's deterministic routing keys).

**F. Identity scope.** Everyone has intra-org identity; only
[A2A](./products/adk-a2a.md) defines cross-org
identity: the AgentCard (name, skills, interfaces, security schemes, JWS
signatures, well-known URI). It is the only serious interoperable
definition, and Vertex + LangGraph + AgentCore + CrewAI all already carry
A2A hooks.

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

These are not mutually exclusive; most products stack two or three.

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

Design decisions the evidence forces, with the industry's answer where one
exists:

1. **Model the trio as three first-class resources**: AgentDefinition
   (versioned), Session (pins a definition version at create), Memory
   (attachable N:M). Do not embed memory or environment in the definition;
   nobody who scaled did.
2. **Version linearly and immutably; sessions freeze.** Allow per-session
   overrides that never write back (Managed Agents), staging/rollback
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
