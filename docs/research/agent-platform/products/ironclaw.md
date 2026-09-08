# IronClaw (NEAR AI): what "agent" means

Part of Agent Definition Research.
Produced by running [RESEARCH_PROMPT](../RESEARCH_PROMPT.md).
Evidence from the `nearai/ironclaw` source: the in-repo architecture and
contract documents plus the implementing Rust workspace. There is no published
product documentation site, so every source below is in-repository. IronClaw is
a Rust monorepo (dual MIT OR Apache-2.0) whose current architecture is called
"Reborn"; the pre-Reborn v1 monolith has been deleted from the tree.

## Source anchors

Sources retrieved 2026-08-05, pinned to commit
[`2ae6621`](https://github.com/nearai/ironclaw/commit/2ae66212fe80208524179047878916dafc0538ee)
(committed 2026-08-05T18:20:35Z). Citations use repo-relative `path:line`.

- `crates/Architecture.md` (1019 lines), the crates-level architecture map.
- `docs/reborn/contracts/`: `kernel-boundary.md`, `capabilities.md`,
  `capability-access.md`, `runtime-profiles.md`, `turns-agent-loop.md`,
  `turn-persistence.md`, `memory.md`, `skills-extension.md`, `triggers.md`,
  `storage-placement.md`, `host-api.md`.
- `docs/reborn/subagent-spawn/{README.md,phase-2-mechanisms.md,phase-3-integration.md}`.
- `crates/contracts/ironclaw_host_api/src/ids.rs`,
  `crates/contracts/ironclaw_loop_contracts/src/snapshot.rs`,
  `crates/domains/ironclaw_threads/src/contract.rs`.
- `profiles/{local,local-sandbox,server,server-multitenant}.toml`,
  `registry/`, `skills/`.

Precedence note the repo states about itself: contract docs under
`docs/reborn/contracts/` and crate-local `AGENTS.md` are authoritative for
behavior-changing work (`crates/Architecture.md:8-9`), and where a frozen
contract has drifted from the code, "**THE CODE WINS**"
(`docs/reborn/target-architecture/CHECKLIST.md:355`). Places where the two
disagree are flagged inline.

## The `agent` noun (primary-source quotes)

- **There is no agent object.** This is the most consequential finding, and it
  is a negative one. `AgentId` is declared as a validated string id alongside
  tenant and user (`crates/contracts/ironclaw_host_api/src/ids.rs:208-210`):

  ```rust
  string_id!(TenantId, "tenant", validate_scope_id);
  string_id!(UserId, "user", validate_scope_id);
  string_id!(AgentId, "agent", validate_scope_id);
  ```

  A repo-wide search for an agent record, an agent registry, or an agent
  definition type (`AgentRecord`, `AgentDefinition`, `agent_registry`) returns
  nothing across `crates/` and `docs/`. There is no create-agent API, no agent
  CRUD, no agent table in `migrations/V1..V34`. The agent is a **scope
  coordinate**, and `validate_scope_id` bounds it at 256 bytes with no path
  separators, control characters, or reserved `__ironclaw_` prefix because the
  id becomes a storage path segment.

- **The scope coordinate is load-bearing everywhere.** `AgentId` is a
  first-class axis of the resource scope model
  (`docs/reborn/contracts/storage-placement.md`), of the transcript scope
  (`ThreadScope { tenant_id, agent_id, project_id?, owner_user_id?, mission_id? }`,
  `crates/domains/ironclaw_threads/src/contract.rs`), of the durable concurrency
  key ("Active-lock key is the canonical `TurnScope`: tenant, agent, optional
  project, and thread", `turn-persistence.md:49`), and of trigger definitions
  ("`agent_id` | Captured agent scope at create time", `triggers.md:3`). What an
  "agent" owns is therefore precise: a partition of threads, memory, triggers,
  locks, and quota buckets.

- **What behaves like an agent is assembled per run, from four separable
  parts.** The architecture thesis names them (`crates/Architecture.md:14-20`):

  ```text
  Products own UX.
  Loops own agent behavior.
  The kernel boundary owns authority, recovery, and side-effect mediation.
  Substrates own durable, reusable primitives.
  ```

  The layers "are not peers. Products and loops are replaceable userland code.
  The kernel boundary is the narrow authority surface they must use for side
  effects" (`Architecture.md:101-104`).

- **The loop is explicitly not the agent's security identity**
  (`Architecture.md:78-80`):

  > The loop is intentionally not the security perimeter. It asks for effects
  > through host ports, and those host ports eventually route privileged effects
  > through the host runtime and `CapabilityHost` boundary.

  The kernel contract inverts the usual framing: "The Reborn kernel is the
  security perimeter. It is defined by what it mediates and secures, not by how
  much product behavior it performs" (`kernel-boundary.md:9-11`). Loop strategy,
  prompt assembly, "routine engines and mission orchestration", skill selection,
  and provider heuristics are all listed as **userland non-responsibilities** of
  the kernel (`kernel-boundary.md:59-69`).

- **Identity is file-shaped data, resolved through a kernel-mediated memory
  substrate.** A reference prompt assembler reads
  (`docs/reborn/contracts/memory.md:250-263`): `BOOTSTRAP.md`, `AGENTS.md`,
  `SOUL.md`, `USER.md`, `IDENTITY.md`, `SYSTEM.md`, `MEMORY.md`, `TOOLS.md`,
  `HEARTBEAT.md`, `context/profile.json`, `context/assistant-directives.md`.
  The kernel does not own their meaning but does own their safety: "identity
  files are primary-scope only"; the stable set (`AGENTS.md`, `SOUL.md`,
  `IDENTITY.md`, `TOOLS.md`, `BOOTSTRAP.md`) "may be considered for default
  prompt assembly"; the personal set (`USER.md`,
  `context/assistant-directives.md`) is "excluded unless the resolved run
  profile explicitly allows personal context"; admin `SYSTEM.md` is admin-scope
  only; and "writes to prompt-injected files are scanned by the safety
  sanitizer" because "these files can affect future execution context"
  (`memory.md:245-278`). Prompt assembly itself is declared replaceable:
  "reference loop prompt assemblers are replaceable behavior, not memory backend
  source of truth."

- **Persistent versus ephemeral.** Both, split cleanly. Persistent: the scope
  coordinate and everything filed under it (threads, memory documents, triggers,
  quota buckets). Ephemeral: the executing thing, which is a `TurnRun` claimed
  by a runner under a lease, executing one loop driver against a checkpointable
  `LoopExecutionState`.

- **Conceptual model.** Agent-as-scope, with agent-as-config assembled onto it
  per run. Not agent-as-identity (no record to point at), not agent-as-process
  (the process is the run), not agent-as-session (one agent has many threads).
  The closest single label: **agent-as-scope-plus-resolved-profile**.

## Subagents

- **Name and shape.** Subagents, spawned through a capability rather than an
  API: `spawn_subagent(flavor_id, task, handoff?)`
  (`docs/reborn/subagent-spawn/README.md`). The public v1 schema is
  blocking-only. The design's central claim is that a subagent is not a new kind
  of thing (`README.md:§5.4`): "a child agent loop is **not** an OS process". It
  is another run on the same runner and driver plane, which the architecture
  lists as a non-goal to violate: "separate subagent execution machinery outside
  the normal runner/driver loop" (`Architecture.md:38-40`).

- **Creation is dynamic, from a static catalog.** The parent spawns at runtime,
  but only from a compile-time flavor table (`General`, `Explorer`, `Coder`,
  `Planner`) whose direction prompts are embedded with `include_str!`. Callers
  choose a `flavor_id`; they cannot author a child's prompt or tool set inline.

- **What the child inherits, and what it does not.** It inherits the scope:
  `tenant_id`, `agent_id`, and `project_id` verbatim, plus `owner_user_id`, and
  only `thread_id` is fresh. It inherits **no authority**: the child begins with
  an *empty* grant and lease set, and the flavor's capability allowlist is
  described as "a surface *ceiling*, not authority". Context is not inherited by
  default: the seed is `Fresh` (goal only) or `Handoff(String)` (a curated
  parent blob re-materialized into the child scope). `Fork` (full parent-context
  copy) is "enum variant reserved, unimplemented" (`README.md:74`) and requesting
  it is denied at runtime (`phase-3-integration.md:677`).

- **The goal lands as a user message, deliberately.** "Never the system message
  — the goal is model-generated and may carry upstream-tainted content"
  (`README.md`). A parent's instruction to a child is treated as untrusted
  input, not as configuration.

- **Nesting is bounded four ways, all before submission.** `allow_nesting =
  false` by default, a depth cap, a per-turn fan-out cap, and an atomic
  `reserve_tree_descendants(scope, root, delta, cap)` against
  `MAX_TREE_DESCENDANTS` backed by a durable `SpawnTreeReservation`. The threat
  model is named: "**Fork-bomb via depth × fan-out.** Three caps — depth,
  per-turn fan-out, per-tree descendants — all enforced before `submit_turn`,
  all rejecting without queuing" (`phase-2-mechanisms.md:1884`).

- **Communication is return-value only, mediated by the host.** The capability
  returns `CapabilityOutcome::AwaitDependentRun`, which awaits the whole child
  set and resolves inline when all children are already terminal. Children get
  their own thread, so there is no shared transcript and no message passing
  between parent and child. Approvals raised by a child surface on the child
  thread, which is why inheriting `owner_user_id` matters.

- **Lifetime coupling is recorded, not silent.** When a parent cancel discards a
  child's result, a durable `SubagentResultTombstone { child_run_id, disposition:
  "discarded_by_parent_cancel", terminal_status }` is written. The parent-child
  link lives on the run (`parent_run_id`, `subagent_depth`,
  `spawn_tree_root_run_id`), not on the thread.

- **Status caveat: off in every shipped profile at this commit.**
  `builtin.spawn_subagent` is deny-filtered via a `TEMP(disable-spawn-subagents)`
  marker in `crates/loop/ironclaw_turn_runner/src/runtime.rs`, per the 2026-07
  status note in `crates/Architecture.md`. The design is detailed and
  contract-frozen; the feature is not on.

## Configuration surface (what, where, why)

Configuration is stratified by *who is allowed to change it and what it can
affect*, and each stratum has a stated reason.

**1. Deployment envelope, in TOML profiles at the repo root.**
`profiles/{local,local-sandbox,server,server-multitenant}.toml` hold coarse
deployment settings: `database_backend`, `[channels] gateway_enabled`/`cli_mode`,
`[sandbox] enabled`, and the proactive-behavior toggles `[heartbeat]`,
`[routines]`, `[hygiene]`. Overrides come from `~/.ironclaw/config.toml` or
environment variables, selected by `IRONCLAW_PROFILE`
(`profiles/local.toml:1-10`). The rationale is deployment-shape independence
without forking the architecture (`runtime-profiles.md:9-19`):

> ```text
> same agent loop
> same CapabilityHost
> same RuntimeDispatcher
> same events/audit/resource model
> different filesystem/process/network/approval backends
> ```

`DeploymentMode` (`LocalSingleUser`, `HostedMultiTenant`, `EnterpriseDedicated`)
crossed with `RuntimeProfile` (twelve presets from `SecureDefault` through
`LocalYolo` and `HostedYoloTenantScoped`) resolves to an
`EffectiveRuntimePolicy` naming the filesystem backend, process backend, network
mode, secret mode, approval policy, and audit mode
(`runtime-profiles.md:36-140`). The invariant is stated as the reason the knob is
safe to expose (`runtime-profiles.md:27-33`):

> ```text
> DeploymentMode constrains the maximum authority available.
> Profile changes backend permissiveness within that deployment.
> Profile does not bypass CapabilityHost.
> ```

**2. Per-run behavior, in a resolved run profile.** This is the closest thing
IronClaw has to "an agent definition", and it is a value captured on a run
rather than a stored template
(`crates/contracts/ironclaw_loop_contracts/src/snapshot.rs:19-41`):

```rust
pub struct ResolvedRunProfile {
    pub run_class_id: RunClassId,
    pub profile_id: RunProfileId,
    pub profile_version: RunProfileVersion,
    pub loop_driver: AgentLoopDriverDescriptor,
    pub checkpoint_schema_id: CheckpointSchemaId,
    pub checkpoint_schema_version: RunProfileVersion,
    pub model_profile_id: ModelProfileId,
    pub capability_surface_profile_id: CapabilitySurfaceProfileId,
    pub context_profile_id: ContextProfileId,
    pub steering_policy: SteeringPolicy,
    pub cancellation_policy: CancellationPolicy,
    pub checkpoint_policy: CheckpointPolicy,
    pub resource_budget_policy: ResourceBudgetPolicy,
    #[serde(default)]
    pub personal_context_policy: PersonalContextPolicy,   // Excluded by default
    pub runtime_constraints: RuntimeProfileConstraints,
    pub runner_pool_id: Option<RunnerPoolId>,
    pub scheduling_class: SchedulingClass,
    pub concurrency_class: ConcurrencyClass,
    pub resolution_fingerprint: RunProfileFingerprint,
    pub provenance: RedactedRunProfileProvenance,
}
```

The stated reason each of these is a *selection* rather than a grant
(`Architecture.md:459-474`):

> Profiles do not grant authority by themselves. They choose bounded surfaces
> and policies that host/kernel services enforce later:
>
> ```text
> profile selects visible capability surface
>   but CapabilityHost still authorizes exact invocation
> profile selects model/context policies
>   but host ports still enforce safety, scope, and redaction
> profile selects checkpoint policy
>   but runner still validates durable checkpoint/result evidence
> profile selects runtime constraints
>   but deployment mode and host runtime policy may only reduce authority
> ```

Note `PersonalContextPolicy::Excluded` as the serde default: the privacy-safe
value is what an old or partial record deserializes to.

**3. Instructions and knowledge, as portable file bundles.** Skills are
`SKILL.md` bundles owned by a first-party in-process extension, and the contract
opens by refusing them authority (`skills-extension.md:11-27`):

> This contract intentionally keeps skills out of the kernel. [...] The
> first-party skills extension is userland code, even when it ships with
> IronClaw and runs in process.
>
> ```text
> skills can provide instructions and supporting files;
> skills cannot grant authority.
> ```

The tree ships roughly thirty of them under `skills/` (`coding`, `code-review`,
`commit`, `delegation`, `llm-council`, `plan-mode`, `product-prioritization`,
various `*-setup` bundles), which is where most of the product's apparent
"personality" per task lives.

**4. Tool and channel inventory, as a static registry.** `registry/` holds
`tools/`, `mcp-servers/`, `channels/`, and `_bundles.json`. Registration is
explicitly not authorization (`capability-access.md:28`): "A registered
capability is only a possibility. It is not authority."

**5. Identity and memory, as markdown under the memory substrate** (the file
list quoted above), with layer scopes (`private`, `shared/team`, custom named
layers) declaring readable/writable flags, writes to read-only layers failing
closed, and optional privacy classifiers able to redirect sensitive shared-layer
writes to private layers with the redirect visible in the write result
(`memory.md:225-240`).

**6. Proactive work, as trigger records.** `TriggerRecord` carries
`trigger_id`, `tenant_id`, `creator_user_id`, `agent_id`, `project_id`, `name`,
`source`, `schedule`, and a materialized `prompt`, managed through
`trigger_create` / `trigger_list` / `trigger_remove` capabilities
(`triggers.md:19-45`). The design constraint is that a trigger does not get its
own execution path (`triggers.md:13`): "It does **not** own a parallel agent
loop [...] A trigger fire is routed into the normal Reborn turn pipeline and then
persists through the same turn, run, and recovery machinery as any other inbound
submission."

**Cross-cutting rationale, stated once and repeated per surface:** a knob may
narrow authority and may never widen it. `TrustClass` "is an authority ceiling,
not a permission grant and not a bypass"; "user-installed packages cannot
self-declare `TrustClass::FirstParty` or `TrustClass::System`"; those ceilings
"do not grant authority by themselves"
(`kernel-boundary.md:94-105`, `host-api.md:293-295`). There is also no privileged
exemption for the vendor's own code: "There is no private back door for shipped
loops or first-party code" (`kernel-boundary.md:47`).

## Binding time

- **Definition time (compile/deploy).** Loop families and drivers are registered
  in code (`DriverRegistry`), subagent flavors are a compile-time table with
  `include_str!` prompts, and the skills/tools/channels registries are on-disk
  inventories loaded at composition. Adding a loop family is a code change with
  a listed blast radius (`Architecture.md:939`).
- **Turn submission time.** The run profile is resolved once, during
  `submit_turn`, as step 3 of the turn flow: the coordinator "persists turn/run
  state, enforces active-thread ownership, resolves the run profile, and emits a
  wake hint" (`Architecture.md:487-493`). Identity/system context, personal-context
  policy, and the model route are resolved in the same window
  (`Architecture.md:748`), and the executor "persists a model-route snapshot
  before invoking the driver" (`Architecture.md:496-497`).
- **Captured, not re-resolved.** This is the binding-time rule that matters
  most (`Architecture.md:440-442`):

  > A resolved run profile is captured on the run so execution can be recovered
  > without re-resolving a different driver or policy after restart.

  So a definition change cannot affect in-flight work: a resumed run replays
  against the profile snapshot, the same `loop_driver` id/version, and the same
  `checkpoint_schema_id`/version. Config edits apply to the next submission.
  Triggers behave the same way at a longer horizon, capturing `agent_id` and
  `project_id` "at create time" (`triggers.md:3`).
- **Mid-run mutability is narrow and typed.** Not the profile: what can change
  mid-run is bounded to steering (a queued user message under
  `SteeringPolicy`), cancellation intent, and approval or auth resolution, each
  of which moves the run through the documented state machine rather than
  editing its configuration. An approval lease is scoped to an exact invocation
  fingerprint and is "resume-only authority" that must be claimed through
  `CapabilityHost::resume_json` (`capability-access.md:88`).
- **Versioning.** Run profiles carry `profile_version`, a
  `resolution_fingerprint`, and a redacted `provenance` record, and checkpoint
  schemas carry their own id and version. That is versioning of the *resolution*
  rather than of a stored agent definition; there is no agent definition object
  to version. `ResolvedRunProfile::legacy_compatibility` exists so turn rows
  persisted before profiles existed still project into the current type
  (`snapshot.rs:60-64`).

## Relationships between nouns

Cardinalities, as the code and contracts express them:

| Relationship | Cardinality and ownership |
| --- | --- |
| tenant → agent | 1:N. `AgentId` is a scope axis under `TenantId`, not an object. |
| (tenant, agent, project?, owner?, mission?) → thread | 1:N. `ThreadScope` is the path prefix; threads live under it. |
| thread → turn | 1:N, but at most one *active*: "One active run per canonical thread is enforced before model/tool side effects" (`Architecture.md`, Key Invariants). |
| turn → turn run | 1:N over retries/resumes; `TurnId` is the accepted message, `TurnRunId` the execution attempt. |
| turn run → runner | 1:1 at a time, by lease. Claim stores runner id plus lease token; heartbeats renew only for a matching, unexpired pair (`turn-persistence.md:88-89`). |
| turn run → loop driver | 1:1, fixed by the captured profile. |
| loop driver → capability invocation | 1:N, every one mediated by `CapabilityHost`. |
| capability → runtime lane | 1:1 per invocation (WASM, script-process, MCP, first-party; system deferred), chosen by `RuntimeDispatcher` *below* authorization. |
| parent run → child run | 1:N, linked by `parent_run_id` / `spawn_tree_root_run_id`, bounded by depth, fan-out, and descendant reservation. |
| child run → thread | 1:1 fresh thread; siblings of the parent's thread, not nested under it. |
| trigger → turn | 1:N synthetic inbound submissions through the normal pipeline. |
| skill → authority | 0. Skills inject instructions and files only. |

Three answers worth stating directly, because the corpus varies most on these:

- **Agent to session: one agent, many threads, and the agent is the scope, not a
  participant.** A thread cannot exist outside an agent scope, and it cannot
  move between agents: scope is baked into the storage path, and `ensure_thread`
  rejects a scope/thread mismatch with `ThreadScopeMismatch`.
- **Agent to sandbox: an agent does not imply an environment.** Containment is
  selected per capability invocation via `SandboxBackend` (`None`, `Srt`,
  `SmolVm`, `Docker`) under the deployment mode's ceiling
  (`runtime-profiles.md:36-48`), and `[sandbox] enabled = false` in
  `profiles/local.toml` shows the same agent running unsandboxed locally. There
  is no long-lived per-agent VM or workspace container in the model.
- **Agent to subagent: ownership without shared state, and death is recorded.**
  The parent owns the spawn tree reservation and the cancel decision; the child
  owns its own thread and starts with zero grants. A cancelled parent does not
  silently orphan a finished child; it writes a tombstone naming the
  disposition.

## Lifecycle

- **The agent is never created or destroyed**, because there is nothing to
  create. Using a new `AgentId` brings a scope into existence implicitly the
  first time something is filed under it; `MemorySeedService` "owns initial and
  upgrade seeding" of `README.md`, `MEMORY.md`, `IDENTITY.md`, `SOUL.md`,
  `AGENTS.md`, `USER.md`, `HEARTBEAT.md` (`memory.md:288-300`), which is the
  nearest thing to provisioning. There is no delete-agent operation; deletion
  exists per thread (`delete_thread`) and per memory document.
- **The run is what has a lifecycle**, and it is a durable state machine
  (`Architecture.md:540-582`): `submit_turn` creates queued work, "but no
  model/tool side effect runs before the process claim succeeds"; a runner claims
  and heartbeats; a capability needing approval or auth writes gate and
  checkpoint refs and the driver returns `LoopExit::Blocked`, keeping the
  active-thread lock; `resume_turn` requeues the same run against the same
  checkpoint; validated exits move to `Completed`/`Failed`/`Cancelled`.
- **Pause and resume are checkpoint-based and evidence-gated.** `LoopExit` is
  "a driver claim, not trusted durable state", and `LoopExitApplier` "validates
  host-owned evidence before mapping the exit to a trusted transition"
  (`Architecture.md:580-582`). An unverifiable exit becomes a sanitized terminal
  failure (`driver_protocol_violation` / `interrupted_unexpectedly`), because "A
  syntactically valid ref is not evidence by itself" (`Architecture.md:644-646`).
- **Crash recovery prefers giving up over guessing** (`Architecture.md:620-632`):

  > ```text
  > runner crashes or stops heartbeating
  >   -> reconciler sees expired Running/CancelRequested lease
  >   -> Running          => terminal Failed (sanitized "lease_expired")
  >   -> CancelRequested  => terminal Cancelled
  > ```
  >
  > Reborn does not automatically retry uncertain side-effecting work after a
  > lost lease — expiry is terminal, and the user resubmits explicitly.

  (`turn-persistence.md:91` still describes expiry as moving to
  `RecoveryRequired` and keeping the lock; `Architecture.md:578-579` says that
  variant "survives only as a legacy variant". The code wins.)
- **Who owns the loop: the product does, in the sense that matters here.**
  IronClaw runs the loop itself (managed), but the loop is a replaceable userland
  plug-in rather than a fixed brain: "Loop diversity is an expected feature"
  (`kernel-boundary.md:71-76`), with lightweight, CodeAct, model-specific, and
  subagent families named. What is *not* bring-your-own is the authority path;
  a custom loop cannot reach the dispatcher directly, and "direct dispatcher
  calls from loops or product entry points" is a listed non-goal
  (`Architecture.md:33-40`).
- **What persists across runs.** Memory and identity documents; the thread
  transcript, summary artifacts, and out-of-band tool-result records; triggers
  and their schedule state; audit and lifecycle events with replay cursors;
  admission reservations while a run is non-terminal. What does not persist:
  grants and leases (per-invocation), secret material (leased once and consumed
  before use), and loop execution state beyond a bounded checkpoint payload.
- **Proactive existence is a deployment toggle, not an agent property.**
  `[heartbeat]`, `[routines]`, and `[hygiene]` in `profiles/local.toml` turn on
  background self-directed work, and `HEARTBEAT.md` is explicitly "volatile
  routine/proactive context, not stable default-loop identity context"
  (`memory.md:270-272`). Whether this agent wakes up on its own is decided by
  the deployment profile, not by anything filed under its id.

## What makes it "an agent" here (our inference)

Our inference: in IronClaw, an agent is **a durable scope coordinate plus a
per-run resolved profile**: an `AgentId` that partitions threads, memory,
triggers, locks, and quotas, onto which each turn binds a loop driver, model
profile, capability surface, and context policy that the kernel then enforces
independently of whatever the loop believes. What makes it an agent rather than
an LLM call is not autonomy or tool use; it is that every effect crosses a
mediated authority boundary that survives the loop being replaced, and that
enough state is journaled for an interrupted run to be resumed or failed
deliberately rather than retried blindly.

Two design commitments follow from that and are worth carrying into our own
work. First, **visibility is not authority, at every layer**: a registered
capability "is only a possibility", a profile "selects visible capability
surface" while `CapabilityHost` "still authorizes exact invocation", a
`TrustClass` is "an authority ceiling, not a permission grant", a subagent
flavor's allowlist is "a surface *ceiling*, not authority", and skills "cannot
grant authority". Layering four independent narrowing surfaces means no single
misconfiguration escalates. Second, **the thing you can replace is not the thing
you must trust**: products and loops are userland and swappable precisely
because the kernel boundary, not the loop, holds the perimeter. An agent
definition in this model is deliberately thin, because a thick one would be
authority in disguise.

## Open questions

- **Where does an agent's identity actually get bound to an `AgentId`?** The
  memory substrate holds identity files and the scope holds a string, but we
  found no artifact that says "agent `X` is this persona with these defaults".
  Is multi-agent-per-tenant a supported product shape today, or is `AgentId`
  currently a single well-known constant per deployment? (`reborn_cli()` defaults
  to `tenant_id: "reborn-cli"`, `agent_id: "reborn-cli-agent"`,
  `crates/app/ironclaw_composition/src/runtime_input.rs:49-71`, which suggests
  the latter for local use.)
- **How is a run profile requested in practice?** `RunProfileRequest` →
  `RunProfileResolver` is documented as a pipeline, but the mapping from product
  intent (a CLI invocation, a Slack message, a trigger fire) to a
  `RunProfileId`, and whether end users can select one, is not stated in the
  contracts we read.
- **Is there any agent-level lifecycle operation at all** (archive, disable,
  delete everything under an `AgentId`)? Deletion exists per thread and per
  document; a scope-wide teardown path is not documented.
- **When does `spawn_subagent` ship?** The design is contract-frozen across
  three phase documents while the capability is deny-filtered off in every
  profile. What is the remaining blocker, and does the empty-grant-set model
  survive contact with real delegation?
- **Mission as a noun.** `mission_id` is a `ThreadScope` axis and "mission
  orchestration" is listed as a userland responsibility, but no contract we read
  defines a mission's lifecycle or its relationship to threads and triggers.
- **Trust-class assignment mechanics.** Ceilings come from "host policy,
  signed/bundled package metadata, or admin configuration"
  (`host-api.md:295`). How an operator actually assigns one, and whether
  signature verification is implemented at this commit, is unclear from the
  contracts.
