# Contract impact: Standard Agents 0.2.0

This analysis reads the [Standard Agents dossier](./products/standard-agents.md)
against the platform infrastructure at local baseline
`3d5354507dd6d937ea70eaa957e7f31332d82dfb`, inspected 2026-11-15.
Upstream is pinned at
`78fa54c14d7125f481f6f05538cce5abb5b7a419`; the
[source ledger](./sources/standard-agents.md) preserves its evidence.

**Recommendation:** keep Standard Agents as a candidate concrete registered
implementation, and use its precise behavioral contracts to review our
adapters. Adopting its agent/thread model as the platform model would require
new decisions. The [prepared contract refinements](#prepared-contract-refinements)
address narrow delegation and recovery gaps identified below; they do not
select a Standard Agents runtime or change the platform's Session lifecycle.

## Evidence boundary

This is a static comparison of this checkout, not other unpublished worktrees
or deployed systems. No provider call, broker fixture, sandbox, SDK package,
or runtime conformance test was executed for the research.

The checkout's `buf.gen.yaml` explicitly excludes `proto/trogonai/agents`,
`session`, `catalog`, `library`, `connections`, `secrets`, and `workflows` from
Rust generation. Constraints and comments in those schemas are contract
evidence, not evidence of an enforcing service. A search of non-generated Rust
found no `AgentConfiguration`, `AgentRevision`, `SessionExecutionPlan`, or
`CreateChildSession` implementation. The tracked source search also found no
Standard Agents integration.

ADRs 0025, 0031, 0032, 0035, 0043, 0062, 0063, 0064, and 0096 are drafts at
this baseline. Their existence and schema elaboration do not change that
status. The [architecture explanation](../../architecture/agent-platform.md)
already distinguishes current transport/runtime foundations from the target
Session shell and model access service.

## What exists here

| Area | Evidence in the checkout | What that establishes |
| --- | --- | --- |
| Durable command execution | `rsworkspace/crates/decider/trogon-decider-runtime` and `trogon-decider-nats`; storage, projector and processor implementations/tests | Reusable native and JetStream command/event infrastructure. It does not implement the proposed Agent/Session services. |
| Sandboxed deterministic deciders | `rsworkspace/crates/decider/trogon-decider-wasm-runtime`; Wasmtime host, empty linker and resource budgets | A concrete deterministic component boundary. It is not a JavaScript `ThreadState.runCode` implementation or a conversational runtime. |
| Scheduling | `rsworkspace/crates/scheduler/trogon-scheduler` and `trogon-scheduler-domain`; dispatch, retry and reconciliation code | Scheduling infrastructure exists. No adapter from Standard Agents `scheduleEffect` to these owners was found. |
| Protocol transports | `rsworkspace/crates/acp/acp-nats-agent`, `a2a/a2a-nats-http`, `mcp/mcp-nats-server` | ACP/A2A/MCP transport foundations. They do not implement Standard Agents' two-side execution, package loading or thread client API. |
| Concrete agent SDK | `rsworkspace/crates/platform/trogon-claude-agent-sdk` | Typed native Claude Code process/protocol integration. Claude Code owns its own loop and model behavior; this is not a generic platform harness or proof of Standard Agents compatibility. |
| Policy evaluation | `rsworkspace/crates/policy/trogon-policy-eval/src/evaluator.rs` | Pure evaluator combining permits, ceilings, holds and narrowing rules. Current facts, enforcement wiring and deployment acceptance remain separate questions. |
| Workload identity and egress | `platform/trogon-workload-identity` and `platform/trogon-egress-proxy` | Governed identity/TLS and policy/audit-aware CONNECT proxy code exists. It does not itself supply a complete provider credential-injection/model service. |
| Provider gateway | `rsworkspace/crates/provider/trogon-provider-gateway/README.md` | Private HTTP/telemetry/health foundation. Its documented current scope excludes inference, provider integrations, OAuth and account pools; inference/management paths return 404. |
| Agent/Session contracts | Excluded `proto/trogonai/agents/agents/v1alpha1` and `proto/trogonai/session/sessions/v1alpha1` | Rich draft declarations for immutable configuration, dependencies, plans, dispatch and outcomes. No runtime conformance follows. |

The implementation rows were checked against source and existing tests, not
re-run. This study makes no current test-pass or deployment-readiness claim.

## Mapping the nouns

| Standard Agents noun | Closest platform concept | Mapping constraint |
| --- | --- | --- |
| `AgentDefinition` | Concrete runtime-native settings inside `AgentConfiguration.settings` | Its two-side structure belongs to that implementation. It is not the platform Agent identity or revision record. |
| Package plus definition closure | Registered implementation release and admitted configuration/dependency artifacts | npm version, package membership metadata and lazy loaders do not establish our exact digest/descriptor/interpreter bindings. |
| Persistent thread | A durable conversation/instance boundary above terminal executions, or an explicitly designed composition of platform Sessions | No one-to-one mapping is established. The thread survives session completion and may change agents. |
| Active `execution` / `flowId` | A candidate mapping to execution attempts or adapter runs | Upstream session stop and execution-loop stop differ. Do not assume either equals platform `CloseSession` or `EndExecutionAttempt`. |
| Child relationship alias | `DelegateDeclaration.binding_name` | The shape is related; upstream side ownership, visibility, create-or-continue and persistent child identity need additional semantics. |
| Subagent session | Child-Session execution under a persistent relationship/instance | `CreateChildSession` supplies a proposed lineage saga, not a persistent restartable Standard Agents child thread. |
| `ThreadState.decide()` | Model inference capability | It produces probabilistic answers. `trogon-decider::Decider` produces deterministic domain events; the shared word is not an integration seam. |
| Thread KV / files | Runtime-specific durable state/artifact adapters | Generic event storage and a checkout reference do not implement the full storage API, isolation, copying or erase semantics. |
| User state / grants | Principal, account, policy and connection owners | A user ID, application role, conversation side and executing Agent are separate identities. |

## Where the contracts align

Our draft `AgentConfiguration` already separates a concrete runtime and its
typed native settings from platform dependency declarations. `AgentImplementation`
names an immutable platform-harness or registered-extension release and a
definition digest. That boundary can describe a Standard Agents interpreter
without making every runtime support its two-side model.

`AgentDependencies` also separates binding names from selected tool/delegate
identity. The Standard Agents distinction between relationship alias and
implementing agent is useful corroboration. Required dependencies and
definition-time failures support our intended fail-closed admission model,
although they do not grant policy authority in either direction.

Both designs distinguish stored history from the context supplied to a model.
Standard Agents gives especially testable snapshot semantics: injecting a
message from a request filter cannot recursively rebuild the same request,
and silent presentation does not remove model-visible history. Our draft
Session store's event log, compaction records, replay/context surfaces, and
redaction contracts provide relevant storage vocabulary, but no matching
context assembler is implemented here.

Durable queues, ordered per-thread execution, explicit failure records,
separate scheduled effects, and copied cross-thread attachments are also useful
acceptance targets for future adapters. Existing NATS primitives can support
parts of those contracts; choosing NATS is not sufficient evidence that the
contracts hold.

## Decisions an integration would have to settle

### Persistent threads and immutable Session plans

Upstream `stopSession()` leaves a thread addressable with its history. A later
message starts another session. Handoff changes the agent assigned to the
same thread. In our draft schemas, `SessionStarted` stores one immutable plan,
and `CloseSession` seals the Session as completed. The plan binds the admitted
revision, implementation and inputs; it cannot be replaced merely because an
upstream thread receives another message.

An adapter needs an explicit durable mapping from upstream thread, child
instance, active session and execution to platform IDs and immutable plans.
A possible design is a persistent conversation owner referencing separately
admitted Sessions. Another is to constrain the supported Standard Agents
profile. Neither is selected here. An in-place handoff that silently changes
the current Session's implementation or Agent revision would violate the
proposed platform boundary.

Related sources: `session_started.proto`, `execution_plan.proto`,
`close_session.proto`; draft [ADR#0031](../../adr/0031-agent-implementation-and-session-plan.md)
and [ADR#0035](../../adr/0035-session-store-decider-aggregate.md).

### Side ownership is routing, not authority

The spec fixes role perspective: user is side B, assistant is side A. The
owning side determines where child traffic triggers the next turn. It does
not identify the authenticated caller, executing principal, or authorization
subject. Human on side A is legal, and custom endpoints separately distinguish
the caller from the thread owner.

Our adapters must preserve those independent identities and use live policy
for child creation, messaging, tool use and outbound publication. An upstream
relationship declaration, `hidden` flag, callback or `exposeAsTool` value
cannot mint a platform grant. Explicit public package endpoints need a reviewed
edge policy rather than automatic exposure through an existing gateway.

Related sources: `AgentConfiguration.execution_subject`,
`dependencies.proto`, `create_child_session.proto`; draft
[ADR#0096](../../adr/0096-connection-use-by-agents.md).

### Parent hooks extend the parent's trusted code

Composing a child automatically activates its required hooks against the
parent's `ThreadState`, including hidden enabled children. This is disclosed
composition behavior, not a side effect of installing a package. It is also
broader than a child obtaining its own isolated storage.

Our draft dependency declarations do not specify equivalent parent-hook
authority. Any implementation admission must include hook code, namespaces,
ordering and disclosures in the exact reviewed behavioral closure. A
dependency pin or tool allowlist alone cannot show what parent state that code
may read or change. Request-context hooks also cannot substitute for mandatory
fail-closed policy/audit enforcement.

Related sources: `AgentImplementation.definition_digest`,
`AgentConfiguration.settings`, `dependencies.proto`; draft
[ADR#0062](../../adr/0062-runtime-owned-settings-and-platform-declarations.md).

### Model fallback, configuration and custody

Standard Agents recommends traversing fallback model names after retries. It
also resolves provider config at request time and permits provider extensions
and per-request transports. Our draft managed-model path requires exact
admitted routes and forbids silent model/provider/credential substitution.

The promising seam is a platform-owned provider/transport adapter bound to the
admitted model-access contract. It must account for every model invocation:
both sides, sub-prompts, bootstrap inference, required `decide()` calls,
provider-supplied tools and any enabled fallback path. Giving package code a
generic provider key would not satisfy our intended custody boundary.

Upstream already has a closer custody contract in its optional user-scoped
credential grants: write-only material, runtime-side injection, origin and
caller policy, and refusal after deletion. That is useful convergence with our
connection-use direction. It does not prove the same isolation for provider
factory config, nor does it define our tenant/project ownership, execution-
attempt binding, current policy source or durable audit acceptance.

The upstream model capability defaults include tools and streaming as true.
Those defaults cannot establish verified runtime support at our admission
boundary. Unsupported behavior must be refused under the exact selected
profile; absence of an error in package loading is insufficient.

Related sources: draft [ADR#0032](../../adr/0032-model-route-and-credential-binding.md),
`AgentImplementation` and the provider gateway's current README. No complete
managed-model service is claimed by this checkout.

### Handler delivery and uncertain external effects

The spec's exactly-once handler wording constrains invocation, ordering and
redelivery after a throw. It does not establish exactly-once remote effects
under crash or ACK loss. An at-least-once processor cannot satisfy it by simply
calling the handler again after recovery.

Our draft `ReserveOperation` binds operation ID and request digest before a
side effect; outcome records and cancellation are separate facts. That is
relevant vocabulary, but there is no implemented bridge from handler delivery
to that ledger. A candidate must define crash points, honest uncertainty,
recovery and operator action before making delivery guarantees. The serialized
parent-handler/blocking-child deadlock also needs an explicit rejection or
documented supported topology.

### Child reuse, results, cancellation and erasure

The Standard Agents child registry needs persistent instance IDs, aliases,
initial arguments, state and working-status text. Blocking creation with a cap
of one must reuse an existing child. Completion can intentionally avoid waking
the parent. Our draft delegation events and `CascadePolicy` express different
facts: parent-first dispatch, child lineage, terminal cascade or independence,
detach and rewind reconciliation.

An adapter must preserve both models rather than deriving instance destruction
from a Session terminal marker. It also needs destination-owned artifact
copies, explicit result delivery deduplication and current read authority.
Upstream mutable message update/delete and subtree deletion require a mapping
to our append-only event, redaction and erasure model. Hiding a row or revoking
a read grant is not a receipt that source files, copied attachments, backups
or derived content were erased.

Related sources: `create_child_session.proto`, `cascade_policy.proto`,
`reconcile_parent_rewind.proto`, `apply_redaction.proto`, `artifact_erased.proto`;
draft [ADR#0035](../../adr/0035-session-store-decider-aggregate.md).

### Deterministic participants and sandbox contracts

Standard Agents allows handler-backed conversation participants, including
agent shapes with no model. Our architecture explanation's directional v1
split puts deterministic coordination in tools/programs or an external workflow
engine; it is explicitly a research direction, not an accepted prohibition.
Supporting a Standard Agents handler graph as an implementation must not
silently commit the platform to a new first-party Workflow aggregate.

Likewise, the existing Wasmtime decider host does not supply the JS/TS isolate,
module linkage, bridged functions, value marshaling and cancellation behavior
required by `runCode`. Installed package execution is a separate trust boundary
from nested sandboxed code execution. Reusing one engine name is not a proof
that these contracts match.

## Prepared contract refinements

The draft Session schemas now carry a `DelegationBindingRef` through child and
external dispatch, child linkage and the write-side fold. It binds the declared
relationship name to the exact parent execution-plan digest, preserving
distinct aliases that resolve to the same target.

Child dispatch also retains `ChildSessionCreation`: the canonical child plan
bytes and digest, checkout and optional candidate. Recovery must use those
original inputs. An existing child counts as a duplicate only after its
verified original creation evidence matches; stream existence alone is
insufficient. Draft [ADR#0035](../../adr/0035-session-store-decider-aggregate.md)
records the cross-record verification and current-admission obligations.

Message-local constraints reject missing or empty new evidence, self-links in
the affected lineage messages, unknown lifecycle values and mixed child/external
delegation state. The Session delegation contract fixtures exercise those
constraints through the shared validator. They do not prove
runtime recovery, source authenticity, digest agreement or authorization.

These changes remain within unpromoted draft packages. Persistent conversation
ownership, reusable child instances, handoffs, parent-hook authority and managed
model integration still require their own decisions and implementation.

## Research priorities and acceptance evidence

The research does not authorize an implementation. If an integration is chosen,
the useful order is:

1. Resolve upstream ambiguities and choose a named supported profile, including
   optional API/sandbox/stream capabilities and any declared restrictions.
1. Decide persistent thread/session/attempt identity and handoff mapping against
   immutable plan admission. Define closure, parent-hook and endpoint review.
1. Establish current registry, principal, policy, model-access and custody owners
   before admitting executable packages. Reuse the platform owners rather than
   placing them inside a protocol gateway.
1. Prove public behavior through a small adapter: invalid definition refusal,
   stable relationship aliases, retained child arguments, create-or-continue,
   session completion without child destruction, and destination-local files.
1. Exercise context snapshot boundaries and silent history, parent notification
   modes, restart/lost-executor state, delivery crash windows, cancellation,
   deletion and policy withdrawal using the actual persistence/transport path.
1. Only then claim compatibility for the tested version/profile. A package
   import, successful model request or runtime-directory listing is insufficient.

No numerical conformance score is assigned: no executable Standard Agents
adapter exists in the inspected tree, and draft schema coverage is a different
kind of evidence from runtime behavior.
