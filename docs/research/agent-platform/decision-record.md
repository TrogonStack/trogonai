# Agent Service: design questions and answers

Part of Agent Definition Research.
Running record of the design discussion that followed the 15-product study.
These are directional decisions, not final commitments. Every answer is
grounded in [the synthesis](./synthesis.md). This record is frozen as
decision-time input: where a conclusion here differs from an accepted
record in the [ADR index](../../adr/index.md), the ADR is authoritative.

## Q1: What should we build?

**An agent registry + evolution service.** The agent is a versioned
declaration that the system itself is allowed to rewrite, through the same
proposal pipeline a human would use. "Self-correcting" is not magic;
in every product that has it, it is a loop: sessions produce evidence, a
reflection process distills evidence into proposals, verification gates them,
and activation mints rollbackable revisions. We build that loop as a
first-class product.

The product opportunity is to connect outcomes to proposed, verified, and
safely activated revisions. Design that loop as a standalone service so it
can sit above any agent runtime.

## Q2: What is an agent?

The synthesis working definition: **a named, versioned declaration of
persona and capability (instructions, model, tools, limits, credentials,
delegation roster) that a runtime instantiates into pinned, resumable
sessions. Agent-ness (the loop, autonomy) is a property of the runtime, not
the record.**

For a *self-evolving* agent, split the declaration into two layers (the
split [Hermes](./products/hermes-agent.md) and [Devin](./products/devin.md)
discovered independently):

- **The charter** (human-owned, slow-changing): identity, purpose, trust
  boundary, tool grants, credential scopes, budget limits, delegation caps.
- **The learned layer** (agent-owned, fast-changing): memory, self-authored
  skills, playbooks, accumulated user/domain model.

Governance rule that follows: the agent freely evolves its learned layer
within the charter; it may only *propose* charter changes.

**Ownership correction:** the later Q15/Q16 audit supersedes the literal
field placement in this early working split. Memory has its own resource; grants, credential bindings,
budgets, and delegation caps live on external planes; charter and learned
layer are governance classes over one complete revision rather than separate
mutable stores.

## Q3: Who provisions it? Who evolves it? How?

**Provisioning:** the operator (human or another agent) creates the charter;
the system provisions everything else. Minimal-creation bar per OpenComputer:
an agent needs only a model and a prompt, on a managed default runtime.
Creation idempotent by name. Environment and memory are separate attached
resources.

**Three evolution actors, different rights:**

1. **The agent itself** writes scoped Memory and contributes evidence for
   learned-layer proposals (Hermes: memory nudges ~10 turns, skill nudges
   ~15, background curator).
2. **Fresh-context verifier agents** gate behavior changes. Cognition's
   production data: fresh-context reviewers beat shared-context ones; the
   proposer never approves its own change.
3. **The human operator** approves charter changes and retains the ability to
   force rollback. Who else may invoke rollback is authorization policy at
   command dispatch, not an Agent-stream invariant. No product in the study
   lets an agent widen its own tool grants or credentials.

**The evolution loop (five steps, all from existing patterns):**

1. **Instrument outcomes** per session (Managed Agents `define_outcome`
   rubric + separate grader).
2. **Reflect on a schedule**: a curator job reads transcripts, outcomes, and
   evaluation evidence, then either records a scoped Memory update or distills
   evidence into a behavior candidate (new skill, prompt edit, dependency
   declaration).
3. **Propose, never mutate**: every behavior candidate opens its own proposal
   with evidence and proposed content; it has no revision number yet.
4. **Verify before activation**: the proposal runs against rubric/evals with
   fresh-context judges; learned-layer approvals may auto-activate, while
   charter changes queue for the human.
5. **Activate with pinning**: activation mints the next immutable revision;
   in-flight sessions keep their revision, only new sessions pick up the
   change, and rollback is one pointer move.

The one sanctioned live mutation: credential rotation (industry-wide).

## Q4: Agent-as-file vs agent-as-record: where does config live?

The framing that cuts through the abstraction: **"agent as a file" versus
"agent as a record" is just: where does the truth live, and who is allowed
to write to it safely.**

**Option 1, agent-as-files, literally** (Claude Code, OpenClaw, eve). An
agent is nothing but a folder:

```
agents/code-reviewer/
├── agent.toml        # name, model, limits, tool grants
├── prompt.md         # the persona/instructions
└── skills/
    └── review-prs/SKILL.md
```

Managing agents = edit files, git commit, ensure the folder exists where the
runtime runs. Great for humans (diffs, PRs, portability). Breaks for our
goal: when the *system* is the author, machine-writes to files/git are racy,
there is no locking, the runtime doesn't know which version a running
session started on, no per-tenant story, and (per the Claude Code dossier)
no versioning primitive at all.

**Option 2, agent-as-record** (OpenComputer, Managed Agents, LangGraph). An
agent is a row/resource with an ID and version.
Managing agents = API calls or a dashboard; `PATCH` mints version N+1;
activation pointers roll back. Machines get optimistic locking (Managed
Agents 409 on version mismatch), audit, multi-tenancy. Humans lose diffs,
PR review, and portability.

**Option 3, agent-as-code** (OpenAI, CrewAI, ADK, Vercel SDK). A
constructor call in the app. Managing agents = normal software deployment.
Ruled out for us: "self-configuring" means the system writes config, and
writing code + redeploying is not that.

**Decision: hybrid, the record is the source of truth, the file layout is
the interchange format.** Mental model: **Dockerfile → image → container.**

- The file layout is the Dockerfile: human-authorable, diffable source form.
- The revision is the image: immutable, numbered artifact minted from the
  files, stored in a registry (OpenComputer: revisions "like a Vercel/Fly
  deployment... never rewritten").
- The session is the container: pinned to the image it started from; new
  images never mutate running containers.

Three products converged on exactly this shape independently: OpenComputer
(repo push of `agent.toml` + `prompt.md` + `skills/` → numbered revision),
Managed Agents (skill folder uploads into versioned agent
resources), eve (compiles an `agent/` directory into a deployed app).

**What "managing agents" means, operation by operation:**

| Operation | Human path | System path (self-correcting loop) |
| --- | --- | --- |
| Create | `agentctl init` scaffolds the folder, `agentctl push` registers it | `POST /agents` with the charter |
| Edit | edit files locally, push → opens a proposal | curator opens a proposal via API |
| Review | diff the active revision against proposed content | fresh-context verifier agents judge the proposal |
| Ship | activate an approved proposal → mints revision N+1 | auto-activate an approved learned-layer proposal |
| Undo | activate the previous revision (pointer move) | same, triggered by regression detection |
| Inspect | list agents, list revisions, see which sessions pin what | same API |

**Carve-out, not everything the agent learns goes through revisions:**

- **Revisioned (the registry):** charter + prompt + skills. Slow-changing,
  behavior-defining, worth proposing, verifying, and activating. These are
  "the files."
- **Not revisioned (separate memory store):** per-session learnings, user
  preferences, episodic memory, domain model. Managed Agents versions
  individual memories inside a store; Hermes keeps MEMORY.md living. If
  every memory write minted an agent revision, version history would be
  noise.

Rule of the whole design: **you never edit the active agent. You propose new
content, and activation alone mints the next revision.** That single rule is
what makes self-correction safe.

## Q5: Is the agent bound to a harness, the provider+model, or both? (HarnessAgent vs Agent)

Three things get conflated and must stay separate concepts:

1. **The brain**: provider + model (`anthropic/claude-opus-4-8`). A
   swappable string.
2. **The loop engine (harness/runtime)**: assembles prompts, wires tools,
   manages context, decides when to call the model again. Claude Code,
   Codex, OpenComputer's `pi`, Hermes's embedded runtime, a LangGraph
   graph, a hand-rolled loop, all harnesses.
3. **The agent definition**: the persona/capability declaration (prompt,
   skills, tools, limits).

The industry answers "is the harness part of the agent?" three ways:

- **OpenComputer: yes, immutable.** Agent = prompt + model + `runtime`
  (`claude` | `codex` | `pi` | `flue` | custom); "runtime is fixed once the
  agent exists; to switch engines, create a new agent." Model is constrained
  by runtime (`anthropic/…` for `claude`, `openai/…` for `codex`).
- **Vercel: implementation detail behind an interface.** `Agent` is just
  `generate()`/`stream()`; `ToolLoopAgent` builds the loop around a raw
  model; `HarnessAgent` runs "a preconfigured established harness, such as
  Claude Code, Codex, or Pi, instead of building the loop yourself."
- **AgentCore: inside the container, not AWS's business.** The agent
  artifact *is* harness+loop, opaque behind `POST /invocations`.

**Decision: no separate HarnessAgent noun; one `Agent` with a `runtime`
field** whose values span the spectrum:

- `runtime: managed-default`: we run a plain tool-loop around the model
  (OpenComputer's "the only thing an agent needs is a model and a prompt").
- `runtime: claude-code | codex | pi | ...`: an established harness
  executes the definition (the HarnessAgent case).
- `runtime: custom`: customer brings the loop behind a turn contract
  (`POST /turn`); we keep sessions, durability, and the event log.

**The test for whether harness belongs in the versioned definition:** does
the artifact change meaning when the loop engine changes? It does: a prompt
and skill set tuned under Claude Code behaves differently under Codex
(OpenComputer's model-per-runtime constraint exists because the coupling is
real). It bites harder for us: **verification results are only valid for
the runtime they ran on.** A revision that passed evals under `claude-code`
proves nothing about the same prompt under `codex`. Therefore:

- `runtime` is recorded in every revision.
- v1 adopts OpenComputer's strict rule: **runtime immutable per agent;
  switching engines = a new agent.** It keeps version history meaningful
  (revision 12 comparable to revision 11 only on the same engine).
- **Model stays a swappable field** within what the runtime can drive, and
  may be overridden per session without minting a revision (OpenComputer
  and Managed Agents both allow this): model swaps change which brain
  executes the agent, not what the agent *is*. Credentials attach to the
  model source (BYO key vs managed), not the harness.

**What we are building, per se: the layer above harnesses.** The registry +
evolution service does not compete with Claude Code or Codex; it treats
them as interchangeable loop engines executing versioned agent definitions.
The agent record says who the agent is (charter, prompt, skills), which
engine runs it (runtime, fixed), and which brain powers it (model,
swappable). Sessions pin all three; the curator evolves the definition
per-runtime.

Naming guidance so this is never re-litigated: don't name anything
"HarnessAgent." The field is `runtime`; an agent *has* a runtime the way a
container has a base image; "agent" is reserved for the declaration.

**Field-strength vocabulary (follow-up clarification).** Fields on an agent
have one of three strengths, and the two headline fields differ:

| Strength | Meaning | Overridable? | Example |
| --- | --- | --- | --- |
| Constraint | part of what the agent *is* | never | `runtime` |
| Default | what a session gets unless overridden at creation, within constraints | per session | `model` |
| Suggestion | advisory; influences decisions, enforces nothing | always | `description` (routing hint) |

`runtime` is a **constraint**, not a "default runtime": no override path
exists (v1). `model` is a true **default**: per-session override is legal
within what the runtime can drive, without minting a revision. And defaults
exist only in the definition: at `SessionStarted` everything collapses to
resolved facts (pinned revision, actual runtime, actual model) that never
change for the session's life (the ledger records what actually ran, never
what was suggested). If runtime ever softens past v1, it moves down this
ladder from constraint to default, at the cost of a per-runtime dimension
on verification verdicts and version history.

One logical persona on multiple engines is NOT plural runtime, it's
sibling agents from one template (`code-reviewer@claude-code`,
`code-reviewer@codex`), each with its own revision history and evolution
loop, tied together by steward routing (later fitness-based) and skill
portability (a skill proven under one runtime is *proposed* to the sibling
and passes that runtime's own verification). Invariant worth protecting:
**one revision binds to exactly one runtime, so a verification verdict
always has a single meaning.**

## Q6: How do we compare runtimes if one agent cannot switch runtimes? How do we group the agents being compared?

**Mental model:** run an A/B test between two separate agents.

For example, create `reviewer-claude-code` and `reviewer-codex` from the same
template. Give both the same charter and rubrics, but bind one to Claude Code
and the other to Codex. Route comparable pull requests to both, score them
with the same rubric, then compare outcome quality and cost per successful
outcome. Because neither agent changes runtime, each revision history remains
clean evidence about one runtime. The conclusion is scoped: one runtime
performed better for this charter and workload, not for every agent.

The steward can use that evidence to send later work to the stronger agent.
That is fitness-based routing.

**Identity:** the two test subjects are separate agents. Each keeps a unique
tenant-scoped `name`, which remains an opaque idempotency key.

**Grouping decision: use labels in v1 instead of adding an `AgentFamily`
entity.** Give each related agent's BehaviorBundle the same selectable label,
such as `family=code-reviewer`. `selectable_labels` is a bounded string map on
the BehaviorBundle. Agent read models expose it from the active revision,
while an existing Session resolves it from its pinned revision. Routing and
comparison views may recognize a small set of keys such as `family`. This
supports tenant-defined grouping without committing to a new
lifecycle-bearing entity. Precedent: Managed Agents `metadata` (16 pairs) and
OpenComputer opaque session metadata.

If families later need their own lifecycle or permissions, promote `family`
to a first-class entity. Adding that entity later is cheaper than removing it
after clients depend on it.

## Q7: Is the agent's content just one system prompt? Can text be replaced?

**Authored content is a bundle, not one string.** The revision holds:
`instructions` (the persona/system prompt: OpenComputer `prompt`, Managed
Agents `system`, Claude Code markdown body, eve `instructions.md`);
`skills/` (on-demand playbooks so the prompt stays small, "without
bloating every prompt"); optionally an `initialPrompt` (Claude Code
precedent: auto-submitted first *user* turn); and a typed
`variables_schema`.
Few-shot examples belong in skills, not fake conversation turns;
multi-message preludes deferred until demanded.

**The model receives an assembly, recorded.** At session start the runtime
assembles the real system prompt (author instructions + platform
injections: environment, memory, skill listings, following the Hermes
composition pattern). Rule: the assembly is deterministic and **ledgered**:
the session records the resolved model input (or digest), so "what did the
model actually see" is always answerable.

**Replacing text, three cases:**
1. **Placeholders bind at StartSession** (ADK `{placeholders}`, CrewAI
   kickoff interpolation): declared in the revision, resolved into session
   facts, then fixed.
2. **Past messages are never edited**: transcript is events; append only.
3. **What the model *sees* can change, because the context window is a
   read model over the transcript.** Compression, skill injection as user
   messages (Hermes's cache-preserving trick), and elision are projection
   changes, not data changes; the projection used is ledgered.

Mid-session instruction changes stay out of v1 (revision pinning + Hermes
cache economics); sanctioned patterns are a new session or the next
revision.

## Q8: Should tools be `required` vs not?

Yes, split the declaration, because it determines *when* failure happens.
Two independent axes, never conflated:
1. **Declaration** (charter, authored): what the agent needs.
2. **Grant** (Grants stream, human-held): what the agent may use.
Effective toolset at session start = intersection.

- **`tools.required`**: fail at the cheapest moment: `StartSession`
  rejects if any required tool lacks a grant, before tokens burn (a PR
  reviewer without `bash` just fails expensively at turn 4 otherwise).
  Pod-won't-schedule-without-required-volume semantics.
- **`tools.optional`**: used if granted, silently absent otherwise;
  instructions degrade gracefully; curator-observed friction becomes
  `RequestGrant` (storyboard 3).

Composition rules:
- Provisioning does NOT hard-fail on missing grants: provision + auto
  grant-escalation; agent sits in derived "pending grants" state
  (AgentRosterView) until the human approves; creation-by-delegation
  keeps flowing; the approval is the go signal.
- Verification verdicts record the **effective toolset** they ran with
  (same logic as recording the runtime): a revision verified with
  web-search granted proves nothing about running without it.
- Undeclared tool attempts still ledger as `ToolCallDenied` (prompt-
  injection audit, unchanged).

Corpus check: no product has this split cleanly (Claude Code `tools` is a
restriction list; Managed Agents tools are config + permission policies;
A2A cards declare capabilities as assertions). This imports
required/optional dependency semantics into the charter.

**Required means wait state, not error.** The tool must be wired before any
session runs; approval can wait.
`StartSession` gives a *typed* rejection (`pending-grants` + escalationId),
never a generic error. The waiting lives on the **work item**, not a
half-started session (no new "blocked" session state): the delegation
parks steward-side, or the integrator subscribes to grant events and
retries on `GrantChanged`. Event-driven, no polling. The ledger narrates
it end to end: `AgentProvisioned → GrantRequested → EscalationRaised →
EscalationResolved → GrantChanged → SessionStarted`, and the gap between
the last two timestamps is the measurable approval latency per agent.

**Mid-session disconnection (follow-up).** Three failures, three answers,
one pinned asymmetry: **the definition pins; authorization never pins.**
Grants are evaluated live at every tool call (revocation is the human's
kill switch, the live-mutation exception, like credential rotation).
1. *Grant revoked midway:* optional tool → `ToolCallDenied(grant-revoked)`,
   session continues degraded. Required tool → cooperative stop,
   `SessionClosed(required-grant-revoked)`, incomplete outcome with costs
   attributed, work item re-parks in the Q8 wait machinery; successor
   session resumes from the persisted transcript on re-grant. No
   "suspended" session state in v1 (sessions only exist when runnable);
   same-session hibernate/resume is the v2 refinement if re-grant proves
   frequent.
2. *Tool endpoint down (MCP outage):* availability, not authorization,
   **a denial is a security fact; an error is an ops fact; the ledger
   never blurs them.** Runtime retries per policy; persistent outage →
   tool-error on the turn, agent adapts or stops gracefully with
   `required-tool-unavailable`; charter budgets bound thrashing; curator
   sees the friction.
3. *Runtime/session crash:* already covered: at-least-once turn
   redelivery, per-session ordering, fail-closed ledger (unrecorded work
   didn't happen and is redriven).

## Q9: Are tools/skills suggestions? Can they be loaded mid-session and promoted later?

**Not suggestions, enforced at every call.** Ladder placement: the
**grant is the constraint** (human-held, evaluated live), the **revision's
declared set is the default**, and **session-time additions are
overrides** (exactly like `model`).

**Mid-session extension: yes, session-local.** Precedent: Managed Agents
("update a session's agent.tools and agent.mcp_servers mid-session...
session-local and do not propagate back"). Rules:
1. Session-local, never writes back to the agent; ledgered as a session
   event.
2. Always inside the grant boundary: extension widens usage within the
   constraint, never past it; widening the constraint is the grant desk.
3. Skills are looser by design: the revision pins the skill *bundle*
   (versions, digest); *loading* one mid-session is a projection choice
   (their whole point); handing a session an extra skill beyond the bundle
   is a session-local override like a tool.

**Promotion: yes, this is the evolution loop's food.** Repeated
session-local extensions and good outcomes are curator evidence → proposal
adds the tool/skill to the defaults → verified → activation mints a revision
for new sessions. Law: **session-local extensions are the mutations, the
curator is the selection, revisions are the heredity.** Ad-hoc
experimentation is the raw material the loop exists to consume.

Caveat (recorded): overridden sessions weaken revision comparability:
they get marked, and the primary metric compares like-for-like; free,
since every extension is a ledger event. Event-model impact: additive, a
`SessionExtended` session event (tools/skills delta) and an override flag
on outcome joins.

## Q10: Who owns the agent? (tenant vs project vs user vs direct allowance)

Four concepts, never blurred:

1. **The wall, tenant (org).** Everything lives in exactly one tenant;
   adversarial isolation is a deployment boundary (decided; corpus-
   unanimous: OpenComputer, Managed Agents, Devin, OpenClaw).
2. **Placement and visibility.** The initial draft used a fixed
   `org | project | user` scope field. Q22 and Q23 supersede that shape:
   the agent is provisioned under `parent`, hierarchy owns placement,
   and visibility derives from hierarchy plus access policy. More specific
   visibility still shadows broader visibility; editing from a narrower
   context creates an explicit fork.
3. **Ownership, lifecycle authority.** NOT the same as scope. Owner = the
   human principal/team whose flag inbox receives this agent's
   escalations and who sits at its grant desk: approves charter-class
   revisions, changes grants, archives, shares. Default = the human
   sponsor behind the provisioner. Machine principals can never own; an
   agent with no human escalation addressee is an orphan the platform
   refuses to create.
4. **Direct allowance, explicit shares.** Access outside inherited visibility
   is an ACL entry, ledgered (`AgentShared`: grantee, capability, by whom,
   why), human-approved via the same grant-desk machinery. Capability tiers:
   **use** (route work / start sessions), **observe** (outcomes, costs,
   revision history), **manage** (delegated ownership powers).

Two load-bearing consequences:
- **Cost follows the session, not the owner**: sessions carry the
  initiator; a shared agent bills whoever asked (Managed Agents vaults
  rationale: product at agent granularity, users at session granularity).
- **Memory remains a scoped resource, not Agent content**: a Session resolves
  memory from its hierarchy and policy context and records the selection or
  snapshot. Sharing an agent never allows one memory scope's contents into
  another Session.

**Corrected post-Q22/Q23:** there is no independent `scope` field on
`AgentProvisioned`. Hierarchy owns placement, the Agent record retains its
accountable owner reference, and `OwnershipTransferred` remains a dedicated
operation if needed later.
**Corrected post-Q15/Q16:** the
`AgentShared`/`AgentShareRevoked` pair does NOT live on the Agent stream:
a share is the owner's stance (access policy), so by the organizing rule
it lives on the **policy plane beside grants** (per-agent access stream,
human-only mutations; grants = what the agent may touch, shares = who may
touch the agent).

**Topology correction:** the final stream-topology decision supersedes the
original single-stream design. The agent registry stream owns
provisioning, activation, rollback, and archive; each proposal stream owns
opening, verdict, and withdrawal.

## Q11: Skill service vs per-agent skill management?

**Both, split as registry vs lockfile: the skill service is the system of
record; the agent only composes, never manages.**

- **Skill service (registry):** skills are first-class, versioned,
  tenant-scoped platform entities with scope, owner, labels, and provenance
  (author: human | curator | import; evidencing sessions). Access shares use
  Q10's separate policy-plane bindings. Precedents: Managed Agents' Skills API
  (workspace-scoped, agents reference by ID), Vercel's `skills add`
  package ecosystem, the synthesis's "the registry is secretly a skill
  economy."
- **Agent = composition via pinning:** the revision pins exact skill
  versions; the bundle digest covers *resolved* content (the npm/lockfile
  split, the revision is the lockfile). A skill update reaches an agent only
  through a proposal verified under that agent's runtime; activation then
  mints the revision. No floating references: a central skill edit must
  never silently change agent behavior without a revision.

Why no private per-agent skill management: portability dies (sibling
cross-pollination needs one entity both agents pin), dedup dies (N agents
evolving N private flaky-tests skills), and the fleet-level fitness signal
("which skills contribute to outcomes across all carriers") dies without
stable skill identity.

Flows, all through existing machinery: curator authors skill → registers in
service → opens a proposal that pins the version → verify → activate. Cross-
pollination = sibling pins the same registry entity, verified under its
own runtime. Session-local loading (Q9) draws from the registry within
scope visibility; repeated good extensions promote to pins. Skill import
(repo/package) lands in the registry, never directly in an agent.

Event-model impact: a Skill stream owns `SkillRegistered`,
`SkillVersionAdded`, and `SkillArchived`. Skill access shares are policy-plane
events, never skill lifecycle events. `ProposalOpened` evidence identifies
skill pins added or removed, each referencing registry `skillId@version`, and
the content digest commits to the proposed definition.

## Q12: What about deterministic agents (state machines, scatter-gather, sequential)?

Corpus dividing line (unanimous): LangGraph "workflows have predetermined
code paths; agents define their own"; Cloudflare "Workflows: linear,
deterministic; Agents: non-linear, non-deterministic"; CrewAI crews vs
flows. Cautionary tale: ADK's Sequential/Parallel/LoopAgent, deterministic
orchestrators dressed as agents, all deprecated in 2.0 for a separate
Workflow concept. Jido's `strategy` field (Direct vs ReAct) shows
determinism as a loop-engine property with the agent noun unchanged.

**Test: who decides the next step? → three rungs, all on existing
machinery:**

1. **No model decision → a tool, not an agent.** Fully deterministic
   pipelines shouldn't burn LLM turns (Hermes `execute_code`: "collapsing
   multi-step pipelines into zero-context-cost turns"). If the state
   machine can be written down completely, write it down, as a tool or a
   plain program.
2. **Model decides within scripted structure → an agent; the structure IS
   its `runtime`.** LangGraph graph, Jido strategy, hand-rolled state
   machine with LLM branch points = `runtime: custom` behind the turn
   contract. Platform-invariant: sessions, pinning, outcomes, costs,
   revisions apply identically; a scripted agent just gives the curator
   less prompt and more structure.
3. **Deterministic coordination ACROSS agents → a workflow, deliberately
   NOT our noun in v1.** Dynamic coordination = steward/coordinator agent
   (already designed). Fixed coordination = external engine (Temporal or
   Vercel Workflows) calling
   `StartSession` as steps, the API is already workflow-shaped (idempotency
   keys, durable sessions, outcomes as step results). Event-driven processors
   can implement the pattern without adding a workflow DSL to the agent
   model.

**Complexity programmed into the agent noun: zero.** No strategy field, no
state-machine schema, no scatter-gather primitives in the charter (v1).
Promotion rule applies: if tenants keep rebuilding the same external
coordination, consider a Workflow noun then.

## Q13: Agent-to-agent coordination: does one agent "require" another?

**Coordination is charter-declared (industry-settled):** a delegation
roster (Managed Agents `multiagent` roster depth-1/max-20; OpenClaw
allowlist off-by-default; OpenAI `handoffs`).

**"May delegate to" vs "requires": Q8's split applied to agents:**
- `delegates.required`: pure coordinators; StartSession typed `pending`
  rejection if no usable delegate resolves + escalation ("provision or
  share an agent matching X"); same wait machinery as required tools.
- `delegates.optional` (the common case): degrade gracefully; repeated
  wished-for delegations are curator friction → proposes provisioning
  (headcount from workload).

**Roster references, default to selectors, not pins:**
- exact `agentId@revision` (Managed Agents-style pinning): reproducibility
  exception only, pinned rosters rot (delegate improves, coordinator
  calls the stale one; delegate archives, coordinator breaks).
- by `name`: scope-resolved (own-shadows-global).
- **by selector (`family=...` label): the default.** Resolved at
  delegation time, where fitness-based routing plugs in (best sibling
  gets the work); fleet improvements flow to coordinators for free.
  Resolution is **ledgered**: the delegation records the concrete
  agent+revision that got the work (defaults in the definition, facts in
  the session).

**Mechanics (all existing machinery):** delegation = a session on the
target agent carrying delegationId/parent ref; parent records child
session ids; results-only return; depth/fan-out caps on the policy plane;
scope+shares (Q10) gate resolution (`use` capability). Lifecycle coupling
is **session-to-session, never agent-to-agent**: parent close →
cooperative cancel of running children by default (Jido `on_parent_death`
is the policy knob if `detached` is ever needed). External delegates =
v2: a roster selector matching an A2A card, same contract, task in,
result out, internals opaque.

## Q14: Should the rubric be part of the agent?

**Defensible part:** the charter carries a rubric reference as a
**default** so every session has a yardstick without the caller supplying
one, and the primary metric needs consistent per-agent scoring. Per the
ladder, it's overridable per work item: outcome definitions legitimately
attach to the WORK (Managed Agents `define_outcome`, Devin's per-session
rubrics).

**The flaw the challenge exposed:** drawn inside the bundle, the rubric
looked revisable like prompt/skills, a Goodhart hazard: **the judged
would own the yardstick.** A curator proposing "rubric → easier-rubric" as a
learned-layer change could auto-activate a lower bar and silently inflate
verification verdicts, the autonomous improvement rate, and fitness
routing. The self-improvement loop must never improve its own grades.

**Fix (three rules):**
1. **Rubrics are first-class registry entities** (Q11 skill treatment):
   versioned, scoped, owned by the operator/QA, never agent content; the
   agent references `rubricId@version`.
2. **The rubric binding is charter-class, never learned-layer.** The
   curator may propose a binding change (with evidence) but it always
   escalates to a human. Rubric *content* versioning belongs to the
   rubric's owner, not any agent's curator.
3. **Comparability is version-scoped:** `OutcomeRecorded` records
   `rubricId@version`; the primary metric compares only within one rubric
   version; a rubric change starts a new baseline, visibly in the ledger.

Event-model impact retained by Q15: Rubric follows the Skill lifecycle-stream
pattern (`RubricRegistered`, `VersionAdded`, `Archived`), and
`OutcomeRecorded.rubricId` becomes `rubricId@version`. Q15 supersedes the
proposed charter binding.

## Q15: Rubrics observe from aside: no binding on the agent at all

**Supersedes Q14's binding location.** A corpus re-check confirms that
nobody binds rubrics on the agent. Managed Agents'
`define_outcome` is a session event; Devin's rubrics attach per session.
The charter `[rubrics]` block was invention, now removed.

**Model: EvaluationBinding, a first-class entity aside the agent:**
`{ selector (agentId | family label | scope | task-type), rubricId@version,
precedence, owner }`, living in the evaluation registry with the rubrics,
mutable by rubric owners (humans) only, ledgered on its own stream. The
grader resolves applicable bindings at outcome time; a delegator's
per-work-item outcome definition is a higher-precedence binding source.
`OutcomeRecorded` carries all applied scores.

**What this fixes over charter-binding:**
1. Goodhart guard becomes **structural**: the agent's revision pipeline has
   no write path into evaluation config, nothing to propose, nothing to
   escalate; the judged doesn't even carry a reference to the yardstick.
2. Multiple observers naturally (correctness + cost-discipline +
   compliance rubrics = three bindings).
3. Selector coverage: `family=` bindings automatically observe future
   siblings; "which agents are unobserved?" is a one-query fleet audit,
   and unobserved agents are excluded from auto-activation (guardrail).
4. Agent history stays about the agent; evaluation churn never mints
   charter revisions.

**Survives from Q14:** rubrics as registry-owned versioned entities;
`rubricId@version` on every outcome; comparisons only within one rubric
version. Primary-metric series keyed by (agent, rubric@version) via
binding resolution; binding stability, visible in the ledger, is what
makes revision N vs N+1 comparable.

Event-model impact: remove `rubricIds` from `AgentProvisioned.charter`;
add Rubric lifecycle events (`RubricRegistered`, `VersionAdded`, `Archived`)
on Rubric streams, plus `EvaluationBindingSet` and `Removed` on each
binding's own stream (human principals
only); verification bench resolves bindings for the agent under judgment.

## Q16: The full "aside" audit: what else was wrongly in the agent?

The final data-ownership decision formalizes this rule and separates
activated revision content, resolved Session facts, Memory, and reusable
Tool and Work contracts.

Generalizing Q15's test: **is this the agent's own fact, or someone else's
stance toward the agent?** The agent stream holds the agent's facts;
every other party's stance (judging, permitting, funding, scheduling,
routing, ranking) is that party's own entity referencing the agent,
usually by selector. Projections never live inside what they observe.

**Stays (the agent's facts):** the Agent record owns name and runtime. The
BehaviorBundle owns description, selectable labels, model default,
instructions, skill pins, variables, and required or optional tool and
delegate declarations. Labels are revision-bound because they are the
selector surface; changing them can change routing, evaluation, and
comparison semantics.

**Moves out (was wrongly drawn in the charter):**
1. **`[limits]` → budget policy on the policy plane.** Spend/token/turn
   caps are the payer's stance (DelegationReceived already carries
   budgetUsd; Devin: per-session ACU caps + org hard caps). Human-owned,
   selector-bindable, never mints agent revisions. Lives with grants:
   both are "what the operator permits."
2. **Delegation caps (max_depth, max_concurrent, allowlists) → policy
   plane.** The need to delegate stays as declaration; tolerated fan-out
   is permission (OpenClaw and Hermes both put these in operator config).

**Keep outside the agent record:**
3. Triggers/schedules/automations: the public corpus models them as separate
   resources (OpenComputer schedules, Managed Agents deployments, Devin
   automations), each referencing the agent.
4. Output routing: a subscription or destination binding.
5. Fitness/priority: observation data belongs in roster projections, not the
   observed entity.
6. Channel/reachability bindings: separate resources.

**Resulting charter (final shape):** identity + engine (runtime, model) +
content (instructions, skill pins, variables) + dependency declarations
(tools, delegates). Nothing economic, temporal, evaluative, or
reputational.

Event-model impact: `AgentProvisioned.charter`
loses `limits` and delegation caps; a Policy plane joins Grants
(BudgetPolicySet, DelegationPolicySet, human principals only); trigger/
schedule and destination entities are future streams, never charter
fields.

## Q17: Do we need the daemon file set (SOUL/USER/MEMORY/...)? The assembly-position model

**No, we need the layers, not the files.** The daemon file set exists because
each layer differs in **injection position, cadence, cache cost, and
visibility**; files were the
single-operator daemons' cheapest serialization of that. Mechanics from
the corpus: system prompt = once-per-session cached prefix (Hermes:
"prompt caching is sacred"); CLAUDE.md re-injected every turn *to survive
compaction*; skills = one-line index in context, bodies on demand,
injected as user messages to preserve the prefix; HEARTBEAT.md loaded
only on scheduled wakes (OpenClaw lightContext); Netclaw strips layers per
audience and gives subagents rules-without-soul.

**Our placement (files dissolve into planes):** SOUL+AGENTS → one
`instructions` doc in the revision (no soul/rules seam needed, our
delegates are real agents with own definitions); IDENTITY → record
fields; USER/MEMORY → memory store (session-start layer; memory writes
never touch the pinned revision or its cache); TOOLS/TOOLING → tool
schemas from grants + runtime envelope + "usage wisdom" as skills;
HEARTBEAT → the scheduling trigger carries its own input; BOOTSTRAP →
template/provisioning input; project CLAUDE.md-style files → workspace
context resolved at session start (scope-layered); skills + agents/*.md →
skill registry + delegates selectors (already designed).

**The assembly-position model (refines Q7):** every layer declares one of
four positions, and the position is part of the ledgered assembly spec:
1. **Cached stable prefix**: charter instructions + skill index; changes
   only via revision.
2. **Session-start bind**: memory snapshot, variables, workspace context,
   grants-derived tool list; resolved once, recorded in the input digest.
3. **Volatile tail / per-turn**: only what must be fresh (time, events,
   steering); kept out of the prefix so the cache holds.
4. **On-demand loads**: skill bodies, fetched content; projection
   choices, ledgered.

Same physics as the daemons, different substrate: they serialize
positions as files in a directory; we serialize them as registry entities
plus a deterministic, ledgered assembly.

## Q18: Per-message chunks (before/after each message)?

**Could: yes, Q17 position 3 (volatile tail). Should: yes, as declared,
revisioned, cadence-limited turn wrappers.** Unnamed but proven in the
corpus: Netclaw wraps every input ("a message arriving at a session actor
**with context-specific instructions**"); CrewAI ships a per-task suffix
("Begin! This is VERY important to you..."); Claude Code re-injects
CLAUDE.md every request (before-chunk, survives compaction) and steers via
appended system-reminders (after-chunks), with hooks as the programmable
form; Hermes contributes the cadence refinement (nudges every ~10/15
turns, not every message, cost).

**Why:** recency. Prefix instructions fade over long sessions (context
rot); safety-critical rules re-asserted near the tail dominate.
After-chunks = strong form (recency works for you); before-chunks =
framing (source, audience, time).

**Rules:**
1. Turn wrappers are **charter content**: `before_message`/`after_message`
   changed through a proposal → verified → activated into a revision; the
   bench can measure whether a wrapper changes outcomes.
2. Volatile tail only, never mutate history, never invalidate the cached
   prefix.
3. **Cadence declared**: `every_turn | every_n_turns | on_trigger`
   (tool-failure, post-compaction); every-turn is the justified exception.
4. Two wrapper sources compose in fixed order: **platform envelope**
   (audience/session/time, Netclaw-style, outermost, never
   agent-controlled) then the agent's declared wrappers (inner).
5. Applied wrappers are part of the ledgered input digest.

**Refused:** runtime self-modification of wrappers mid-session, "I should
remind myself X every turn" is a curator observation → proposal
(the standard promotion pipeline), never a live mutation.

**Who does it (clarified):** the agent never *performs* injection. The
revision **declares** wrappers; the runtime adapter **applies** them
mechanically at assembly time (same code path as the Q7 session-start
assembly, running at position 3); the model **experiences** the wrapped
input without knowing the mechanism exists; the curator **evolves** the
declarations via proposals and activation. Definition declares, platform enacts,
ledger records.

## Q19: Extend a mutable registry or start from the event model?

**Start from the event model.** The product requires complete provenance:
curator evidence, verification history, activation decisions, rollback, and
cost per outcome. Without events, those become bolted-on logs rather than
the projections that drive the evolution loop.

Retrofitting provenance later would require replacing the storage model and
migrating consumers after the contracts are live. Extending a mutable CRUD
registry is defensible only if the product shrinks to bookkeeping; verified
self-evolution makes events load-bearing from the first release.

## Q20: How does an agent get access from the beginning?

Three paths, in order of commonality:

1. **Placement, not shares.** "Give project B access from the start" =
   provision the agent under project B's `parent` in the hierarchy. Scope
   and visibility derive from hierarchy and policy, as finalized by Q22 and
   Q23. Shares exist only for cross-placement exceptions.
2. **Sequential commands, deliberately non-atomic.** ProvisionAgent (Agent
   stream) then ChangeGrant xN / ShareAgent (policy plane, human
   principal). No cross-stream transaction, and none needed: the Q8
   pending-grants wait state makes the gap safe by design (the agent
   exists, shows as pending, StartSession typed-rejects until grants
   land). Human provisioners issue both back-to-back; the steward's grant
   needs become GrantRequested + escalation.
3. **Templates = pre-approval at authoring time.** A human blesses a
   template once, INCLUDING its grant preset and ceilings; provisioning
   from it applies those grants automatically with provenance pointing at
   the template's blessing. Provisioning must reject machine principals
   that exceed the template's grant ceilings. The human-only invariant
   survives: approval moved
   from per-agent to per-template. Beyond-ceiling needs drop to path 2.

Stream-authorship rule for implementers: **ProvisionAgent emits only
AgentProvisioned, never policy events.** Even on the template path, grant
events are appended by the policy plane's own command (template preset as
input). Streams never write each other's facts.

## Q21: How do Google APIs do it? (API-surface conventions)

Google's AIPs answer the Q10/scope question at scale: **placement lives in
the resource name** (AIP-122/133: `tenants/{t}/projects/{p}/agents/{a}/
revisions/{n}`; creation is `POST /{parent}/agents`, so placement = which
collection you create into; Vertex already does this for agents:
`reasoningEngines/{re}/runtimeRevisions/{rev}`), and **ownership/access
lives aside as IAM policy** attached to the resource name and inherited
down the hierarchy, our stance-plane rule discovered independently.
Their grammar rhymes throughout: etags = our WritePrecondition;
soft-delete/undelete = archive-not-delete; custom methods (`:activate`,
`:rollback`, AIP-136) = lifecycle commands not PATCHes; the standard `labels`
map informs Q6's bounded selector syntax, not ownership; our
`selectable_labels` live on BehaviorBundle. Revisions-as-child-collection
with aliases (AIP-162) = our activation pointer.

**Decision:** adopt AIP-style resource names for the API surface (bilbao
is ConnectRPC + protobuf; AIPs are built for proto), while hierarchy events
own placement. The resource name is a *rendering* of tenant + parent path +
name; a hierarchy move changes that rendering, and references-by-id survive,
avoiding Google's one sharp edge (name-encoded placement makes moves nearly
forbidden). The Agent ledger does not duplicate placement as `scope` data.

## Q22: What if tenants have projects, orgs, teams, users, or whatever?

**Don't enumerate levels, one recursive Container, tenant-drawn trees.**
Q10's `scope: {level: org|project|user}` quietly hardcoded three levels;
superseded. Industry-unanimous move for unknown tenant structure: Google
Cloud folders (untyped, nested; IAM inherits down), AWS OUs, and
Zanzibar/SpiceDB (no levels at all, only parent/member relations;
authorization resolves through whatever graph the tenant builds). The same
reason that favors open-ended labels applies to tenant-defined hierarchy.

**Model (as first drafted):** `Container { id, parentId?, kind, name }`
on the tenancy plane; tenant root is a container; agents carry a single
`container` ref (one canonical parent, AIP-124 style). **`kind` is a
label, not a schema**: the platform interprets exactly two things: the
parent chain and membership. Consequences:
- Shadowing generalizes to *nearest ancestor wins* up the chain.
- Policy/evaluation bindings select by subtree and inherit down
  (IAM/Zanzibar move).
- "User scope" = a container of kind `user-home`; org/project/team/
  whatever collapse into one mechanism.
- Q21 names render the container path; references stay by id; moves
  survive.
- The placement conclusion survives the final topology's later stream
  split: an agent stream with four deciders and a proposal stream with
  three. Placement is still an opaque reference validated outside those
  deciders.

Q6 promotion rule guards the reverse: promote a `kind` to a real concept
only when the platform must *interpret* it (e.g. billing-per-project):
promotion cheap, demotion breaking.
**Supersedes:** Q10's fixed scope levels (the placement/ownership split
itself stands).

**Cross-platform verification:** all three clouds
converged on this exact shape, untyped recursive middle containers
(GCP Folders, AWS OUs, Azure Management Groups; depth capped single-digit
everywhere), policies inheriting down, and **exactly one promoted
container kind** where billing/isolation anchor (GCP Project, AWS
Account, Azure Subscription). Our promoted kind is the **tenant**, same
architecture, one level rotated. Kubernetes is the cautionary tale for
skipping the tree (flat namespaces; hierarchy bolted on later via HNC);
Zanzibar is the pure-relations extreme that underlies our membership
resolution anyway.

**Inheritance semantics fork (recorded):** GCP IAM inherits
*additively* (children gain, never lose); AWS SCPs inherit as *ceilings*
(parents bound children). We need both, and the planes already split that
way: **grants are additive down the tree; budgets/caps are ceilings down
the tree.**

**Q22 addendum, the container dissolved (mistake chain recorded).** The
decision was worked into a formal ADR
([straw-hat-team/adr #4761776210](https://github.com/straw-hat-team/adr/pull/345),
finalized and merged 2026-07-09), and field-by-field review shrank the model
three times:
1. `Container { id, tenant, parent?, kind, name }`: first draft, five
   fields (mistake: carried derivable and decorative fields).
2. `Container { id, parent? }`: tenant is the root ancestor, kind is a
   label, name is an API-surface slug (mistake: the parent still drawn as
   a node property).
3. **No container entity at all.** Add/move commands validate *tree*
   invariants (parent exists, no cycle, depth cap), so per the
   per-command-state rule the tree is the consistency unit: a per-tenant
   event-sourced `Hierarchy` stream (NodeAdded/NodeMoved/NodeRemoved),
   with the current tree as a projection. GCP/HNC parent pointers are
   projection fields, not model.
**Final semantics: a container is an opaque id string.** Meaning accrues
around it from other planes (hierarchy events mention it, annotations decorate
it, bindings select it, resources reference it). Validity is checked
where the id is *used* (command dispatch validates against the hierarchy
projection), never inside deciders (purity rule). Deciders unaffected
throughout.

## Q23: What do we call the placement field?

**`parent`.** Verified against primary sources: GCP Project has field
`parent` ("A reference to a parent
Resource. eg., organizations/123 or folders/876") plus `projects.move`
("Move a project to another **place** in your resource hierarchy, under a
new resource parent"); AWS `MoveAccount(AccountId, SourceParentId,
DestinationParentId)` + `ListParents`, with docs calling the parent "the
destination container"; AIP-124 confirms "at most one canonical parent"
but does NOT prescribe the field name (earlier claim overstated).

**Verified nuance:** the explicit `parent` field is the convention for
hierarchy nodes and for **movable** resources; plain leaf resources vary
(GCP leaves encode placement in `name`; Kubernetes leaves use
`metadata.namespace`; AWS leaves use tags, the AWS Account stores no
parent at all: parenthood is queried, which independently validates the
tree-outside-the-node ADR). Our agents are the GCP Project analog exactly
(movable registry resources), which is the case where Google chose an
explicit `parent` field + move method.

First draft chose `place` (the ADR's prose word); it was rejected as
confusing, and convention wins. The collision worry (parent
sessions/agents in delegation) dissolves under the rule everyone already
follows: **bare `parent` on a resource always means placement; kinship is
always qualified** (`parentSessionId`, `parentAgentId`, as the event
model already spells them). Still disqualified: `container` (OCI
collision), `scope` (executed in Q22), `location` (regions).

**Q23 addendum (spelling rule, ADR#6310044131 rule 4):** `parentId` /
`parent_id` are disallowed. Placement is spelled exactly `parent`;
kinship is `parent<Type>Id` (`parentSessionId`). The spelling carries the
semantics: unsuffixed = placement, type-plus-Id = kinship, bare middle
form unwritable. Consequence in the event model: the hierarchy
stream's `NodeAdded { nodeId, parentId }` becomes
`NodeAdded { nodeId, parent }` (a node's parent IS its placement).

## Q24: The changeClass taxonomy (governs review and activation)

**Topology superseded and field ownership refined by the final
decisions:**
`ProposalOpened` now records the `changeClass` derived from its canonical
typed difference. After approval, the application uses it to choose automatic
learned-layer activation or human-gated charter activation. The taxonomy
otherwise remains:

**learned-layer (auto-activate after verification):** instructions/prompt,
turn wrappers, skill pins, `tools.optional`,
`delegates.optional`, `description`, labels except well-known keys.
Rationale: behavior content the loop exists to evolve; widening a
*declaration* grants nothing (grants still gate, Q8).

**charter (escalates to a human):** `tools.required`,
`delegates.required` (changes the runnable condition; can brick session
starts), `model` default (capability/cost shift; v1 conservative), the
`family` label (moves the agent between comparison/routing groups), and
`variables_schema` (a session-caller interface; v1 treats every schema change
as charter-class).

**not revision changes at all:** `name` (idempotency key), `runtime`
(immutable; new sibling agent), `parent` (a move: its own audited
operation), `owner` (a transfer: its own audited operation). The Agent
stream's revisions carry behavior only; placement and accountability
changes never mint revisions.

## Q25: Labels align with ADR#5177934677 (annotations), with a selector carve-out

The labels decision aligns with
[straw-hat-team/adr#338](https://github.com/straw-hat-team/adr/pull/338)
(ADR 5177934677, "Annotations and Transient Annotations", **finalized and
merged 2026-07-09**, with the selector clause made conditional during
finalization). Adopt it for opaque metadata, with the selector carve-out the
merged ADR permits.

- **Adopt the annotations spec wholesale** for all decorative/opaque
  metadata we previously called labels: node `kind`, `team`, `vertical`,
  display names, tenant-invented keys. Kubernetes key syntax, DNS-prefix
  ownership, annotations vs transient_annotations lifetime on events. Our
  Q6 rationale is that ADR's opening premise.
- **The carve-out:** ADR#5177934677 rejects the labels/annotations split
  conditionally: "we do not have a platform-level selector engine." The
  agent platform IS one: rosters resolve family= selectors (Q13),
  evaluation bindings select by label (Q15), fitness groups by family
  (Q6), the steward routes on it. And selector values need bounded,
  matchable syntax, while annotation values are explicitly free
  (JSON-encoded blobs). So each BehaviorBundle keeps a **minimal
  `selectable_labels` map for selectable keys only** (family,
  binding-matchable keys) with
  label-value syntax constraints, as the downstream ADR that spec invites.
  Agent projections may expose the active revision's labels, but the Agent
  record never stores an independently writable copy. A label change follows
  Q24 and therefore requires a Proposal and a new AgentRevision.
- Consequence: `kind` moves from "labels" to annotations (it was never
  selected, only displayed); `family` stays a label (it is selected).

## Q26: The domain layer must not know principal kinds

A Human/Machine principal enum inside `activate_revision` would be a
layering violation, the same class as rubrics-in-the-charter one level down:
**the domain may know principal IDENTITY (an opaque value its
own events recorded, compared for equality), never principal KIND** (an
ontology only authentication holds; aauth,
[ADR#0017](../../adr/0017-aauth-agent-authentication.md)).

Layer mapping, corrected by the final stream topology:
- Proposal-stream deciders: proposer != approver (identity equality from
  stream facts), at most one verdict, a withdrawn proposal takes no verdict,
  and only the author withdraws.
- Registry-stream deciders: revisions mint linearly at activation, archived
  agents reject activation, the same proposal cannot activate twice, and a
  rollback target must have been previously active, including revision 1 made
  active by provisioning.
- App-level gates at command dispatch (where aauth knows kinds):
  charter-class activation requires a human; grant/policy/evaluation
  mutations are human-only (invariant 3). The decider records
  activated_by as opaque and documents the upstream precondition.

Consequence: the purity rule generalizes: deciders consume stream facts
and opaque inputs; every ontology from another plane (grants, tree
validity, principal kind) is proven upstream and arrives as data.

## Settled design core after Q16

1. **The charter is minimal** (Q16 final shape): identity + engine
   (runtime constraint, model default) + content (instructions, skill
   pins, variables) + dependency declarations (tools/delegates,
   required/optional). Nothing economic, temporal, evaluative, or
   reputational.
2. **Everything else is a stance on its own plane**, selector-bound,
   independently human-owned: policy (grants, budgets, delegation caps),
   evaluation (rubrics + bindings, the judged never owns the yardstick),
   scheduling, routing, reputation (projections only).
3. **Definitions pin; authorization never pins.** Sessions freeze their
   revision; grants/policies evaluate live at every call.
4. **The loop's integrity invariants:** proposer ≠ approver; machines
   never widen grants; the judged never owns the yardstick; unobserved
   agents don't auto-activate.
5. **Flexibility lives in the session** (extensions, overrides, all
   ledgered), and the curator promotes what works: session-local
   extensions are the mutations, the curator is the selection, revisions
   are the heredity.
6. **Coordination:** selector rosters (family labels) with
   required/optional delegates; session-to-session coupling only;
   determinism routes to tools, runtimes, or external workflows, never
   the agent noun (ADK's deprecation is the documented cautionary tale).

## Open questions (to revisit)

Resolved since first written: multi-tenancy (Q10), rubric authorship
(Q14 to Q15: registry-owned, selector-bound), skills placement (Q11), and
the change-class taxonomy (Q24).
Still open:

- Is runtime-immutable-per-agent too strict past v1 (per-revision runtime
  with migration evals instead)?
- A2A AgentCard export: at agent level or revision level?
- The template system (Q16 identified it as the biggest undesigned
  dependency).
- Evaluation binding precedence rules (default vs work-item vs multiple
  bindings on one outcome).
