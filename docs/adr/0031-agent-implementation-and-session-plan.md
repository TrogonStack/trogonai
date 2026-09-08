---
number: "0031"
slug: agent-implementation-and-session-plan
status: draft
date: 2026-07-18
---

# ADR#0031: Agent Implementation and Session Plan

## Context

An Agent needs a precise answer to two questions:

1. Which implementation interprets this Agent definition?
2. Which behavior can the platform inspect and enforce for this Session?

Terms such as runtime and harness have different meanings across products.
The configuration's `runtime` binding must identify the exact implementation
that interprets its settings, rather than conflating behavior with hosting.
Otherwise a supposedly immutable Agent revision can acquire a newer
implementation when a Session starts.

This decision uses a smaller model:

    Agent
      stable identity

    AgentConfiguration
      exact runtime version
      one runtime-owned typed settings message
      supported platform-owned declarations

    AgentRevision
      immutable numbered binding to one AgentConfiguration

    SessionExecutionPlan
      immutable record of the exact revision, implementation, settings,
      dependencies, and supported capabilities admitted for one Session

The platform-managed harness loop is the normative v1 implementation in this
model. Codex and Claude Code are possible future edge integrations, not the
source of the core Session schema. An OpenAI or Claude model is a separate model
selection when the selected runtime exposes that control. A process, container,
microVM, or remote service may host an implementation, but hosting is not itself
an Agent implementation.

OpenClaw uses its own precise product vocabulary: an agent runtime owns a
prepared model loop, and a harness implements that runtime. The platform does
not copy those nouns into its core model. A future OpenClaw adapter would map
the exact behavioral component to AgentImplementationVersion and preserve
OpenClaw's native terms inside typed OpenClaw configuration.

This ADR refines the ownership described by draft
[ADR#0025](./0025-agent-definition-data-ownership.md). The exact
implementation belongs to AgentConfiguration, not to the stable Agent
identity. Changing implementation kind, version, configuration, or model is a
reviewed behavior change that produces a new AgentRevision.

It also refines the frozen
[agent-platform decision record](../research/agent-platform/decision-record.md),
which used `runtime` as an immutable Agent field and allowed a Session to
override the model. That record remains historical research input. If this ADR
is accepted, its exact implementation binding and capability-scoped model pins
become the normative decision.

The repository already establishes adjacent constraints:

- [ADR#0009](./0009-protocol-buffers-wire-contracts.md) requires typed,
  versioned [protobuf](../glossary/protocol-buffers) contracts and prefers explicit messages over untyped
  maps.
- [ADR#0023](./0023-secret-management-and-key-custody-direction.md) keeps
  secret values behind the secrets service.
- [ADR#0024](./0024-agent-platform-stream-topology.md) places a durable fact
  in the [stream](../glossary/stream) whose ordering is required for its invariant.
- Draft [ADR#0032](./0032-model-route-and-credential-binding.md) separates
  exact model selection from provider connections and credentials.
- The generated protobuf types use unknown_fields=false. Older readers can
  discard unknown arms and unknown fields, so version skew must fail closed.

This ADR defines logical ownership, Session admission, the platform harness
boundary, constraints on future implementation adapters, remote verification,
and protobuf modeling. It does not select a container orchestrator, define a
general hosting resource, or move provider credentials into Agent
configuration.

Draft [ADR#0062](./0062-runtime-owned-settings-and-platform-declarations.md)
defines the general Agent boundary: one exact runtime binding and one typed
settings payload, with common declarations for supported platform integrations.
The platform harness and managed-model path below are a specific execution
contract. A valid configuration for another runtime does not automatically
qualify for that contract or require the same native controls.

## Decision

### 1. Pin the runtime and its settings in AgentConfiguration

Use these normative logical records:

    AgentImplementationVersion
      implementation_version_id
      kind = platform_harness | registered_extension
      platform_harness:
        harness artifact references and digests
        harness contract version
      registered_extension:
        immutable extension version reference and definition digest
      settings type URL and descriptor-set digest
      settings contract version and validator
      supported platform integrations and inspection capabilities
      definition_digest

    AgentImplementation
      exact AgentImplementationVersion reference and definition digest

    ModelSelection
      exact ModelVersion reference and definition digest
      deterministic model parameters

    AgentConfiguration
      runtime -> AgentImplementation
      settings -> one google.protobuf.Any accepted by that exact version
      description and selectable labels
      platform-owned variables, skills, and dependencies
        (declared integrations require support from the pinned adapter)
      configuration_digest

    AgentRevision
      agent_id
      revision_number
      configuration_ref
      configuration_digest
      source

`ModelSelection` is a typed projection for platform-managed model access, not
a mandatory field on the general AgentConfiguration. A pinned adapter derives
it from the runtime-owned settings when it can prove exact models, roles, and
parameters and enforce them during execution. The author does not maintain a
second platform model selection beside the native settings. A runtime with no
caller-controlled model selection is not thereby model-free; it may manage
models internally and be ineligible for the managed-model capability.

AgentImplementationVersion describes a reusable immutable release. For the
built-in arm, its definition digest commits to the platform harness artifacts,
supported contracts, and implementation capabilities. A mutable tag such as
latest is not an exact version. A registered edge extension pins any additional
product and adapter artifacts inside its immutable extension version, not in
the built-in harness fields.

Every behavior-significant artifact is pinned transitively. This includes a
bundled CLI or SDK, plugins, nested components, composition rules, and generated
assets when they can affect implementation behavior. Recording only a wrapper
or launcher version is insufficient.

The runtime binding inside AgentConfiguration selects one exact version. Its
single settings envelope contains the complete message accepted by that
version, including native models, parameters, instructions, and context
controls when supported. The AgentConfiguration digest commits to the binding,
settings type and admitted bytes, and platform-owned declarations. AgentRevision
binds the same configuration and digest after activation.

ModelSelection identifies an exact versioned model catalog record, not a
display name, mutable provider alias, auto value, or provider credential. Its
parameters derive from the admitted runtime settings, including defaults whose
meaning is fixed by the pinned runtime contract. The managed-model capability
requires the implementation to honor that exact projection without a harness
default or fallback replacing it. If the adapter cannot prove the projection or
prevent substitution, admission rejects that capability.

For the managed-model capability, every auxiliary model is derived under a
typed role from the same settings and pinned implementation contract. An
implementation release cannot add a hidden platform-authorized model call.
Changing the runtime settings or version requires a new AgentConfiguration and
AgentRevision. A settings contract may permit native choices that this
capability cannot admit; configuration validity and Session eligibility are
separate judgments.

The stable Agent record contains no implementation family. Changing from
Codex to Claude Code is a Proposal and a new AgentRevision. Governance may
require a sibling Agent for a particular change, but the storage model does
not silently create that rule.

For example, AgentRevision 7 can pin an exact runtime release whose settings
select an exact model. If its adapter supports platform-managed model access,
a Session may resolve that derived selection through an admitted provider
connection and [credential binding](../glossary/credentialbinding), but it cannot replace either pin.
Moving the same plan to another conforming host does not create a revision.
Selecting Claude Code, a newer Codex release, or another model requires a new
AgentConfiguration and AgentRevision.

The following boundary decides where a value belongs:

- If the Agent author or implementation controls a value that can change
  prompts, context, tool or delegate sequencing, stopping behavior, model
  choice, or model-visible state, it belongs in the immutable implementation
  version or AgentConfiguration.
- If a value adapts an already-pinned ModelSelection to an external provider,
  it belongs to the exact [ResolvedModelRoute](../glossary/resolvedmodelroute) in the immutable
  SessionExecutionPlan. Because provider translation can affect observable
  behavior, verification evidence is scoped to that route and changing it
  requires a new Session.
- If it only places, isolates, starts, monitors, or stops the exact pinned
  implementation without changing those semantics, it is deployment or
  Session launch state.

An OCI image may therefore be an implementation artifact when it is the
immutable distribution of the pinned platform harness. A future registered
extension pins a product image inside its extension version. The cluster, node,
process supervisor, filesystem allocation, and network placement that run it
remain hosting details.

### 2. Bind one immutable SessionExecutionPlan

Before a Session becomes runnable, create one SessionExecutionPlan:

    SessionExecutionPlan
      session_id
      agent_revision_ref
      agent_configuration_ref + configuration_digest
      agent_implementation_version_ref + definition_digest
      runtime_settings_type + runtime_settings_digest
      effective_harness_configuration_digest
      admitted platform-model capability, when required:
        derived primary_model_selection + resolved route
        derived auxiliary_model_selections + resolved routes
      resolved variable bindings
      resolved tool, delegate, memory, and skill versions
      work contract and input references
      platform-harness compaction policy, when that execution contract applies:
        resolved trigger thresholds, input, summary, guidance, retained-tail,
        bounded-pass, and non-summary encoding budgets,
        covered-input contract version, and producer role
      harness contract version
      session-plan contract version
      resolved-model-route contract version

The implementation and settings are bound to the pinned AgentConfiguration.
When platform-managed model access is required, admission runs the pinned
adapter against those settings and records its verified ModelSelection
projection. It never chooses a different implementation, model, or parameter.
V1 has no Session override for those admitted values.

The resolved model routes add the [provider connection](../glossary/modelproviderconnection), non-secret credential
binding reference, exact provider driver, and protocol required to serve each
ModelSelection. They cannot substitute another model. Attempt-scoped
[model-access grants](../glossary/modelaccessgrant), secret values, and short-lived provider credentials never
enter the plan.

The platform harness's own settings pin the exact compaction trigger thresholds,
input, summary, guidance, and retained-tail budgets, bounded-pass limits,
covered-input contract version, and producer role. The resolved policy also
sets encoded-length bounds for every variable non-summary `Compacted` field and
every decider envelope or header value the append carries. Admission rejects an
unbounded or unsupported value source and copies the accepted limits into the
immutable SessionExecutionPlan. The selected AgentImplementationVersion
declares the policy contract and supported limits. The producer role selects
either the pinned primary ModelSelection or the exact pinned auxiliary
ModelSelection under the typed role `compaction`. It cannot name an arbitrary
model.

The managed-model projection is a required capability of the platform harness
path described here. Another runtime's AgentConfiguration can be valid without
it, but cannot enter a Session requiring managed-model guarantees. Compaction
controls likewise belong to the platform harness settings and execution
contract, rather than becoming universal Agent fields. A different runtime's
checkpoint or compaction mechanism does not gain platform semantics merely by
sharing the general configuration envelope.

Admission proceeds in this order:

1. Load the requested AgentRevision and verify AgentConfiguration bytes and
   digest.
2. Load the exact AgentImplementationVersion and verify its definition and
   harness artifact digests. A registered edge extension verifies its own
   pinned product and adapter artifacts under the extension contract.
3. Validate the settings type and complete payload against the exact runtime
   settings contract. Validate each present platform declaration and require
   the pinned adapter's integration support; optional resource resolution does
   not permit unsupported declarations to be ignored.
4. If the Session requires platform-managed model access, require its pinned
   adapter capability and derive verifiable primary and auxiliary
   ModelSelection values from those settings. Reject unsupported or hidden
   model control for that capability.
5. For that capability, resolve an authorized provider route and
   CredentialBinding for each exact model under
   [ADR#0032](./0032-model-route-and-credential-binding.md).
6. Resolve tools, delegates, memories, variables, work input, and other
   Session dependencies.
7. Verify every required Session capability and, for managed-model access,
   every admitted model protocol.
8. Build the exact harness configuration projection and its expected digest.
9. Store the canonical SessionExecutionPlan bytes and digest atomically with
   SessionStarted.
10. Authorize the first launch only after observing that durable start fact.
    Where managed-model access is admitted, create its grants only after Ready
    binds them to a specific ExecutionAttempt.

A missing required dependency, or an ambiguous, unavailable, unauthorized,
revoked, incompatible, or digest-mismatched admitted value, rejects Session
start with a typed failure. Optional dependencies may remain absent under their
declared resolution rules. Admission never falls back to another implementation
release, model, provider account, credential, or extension.

Across admission, Ready, recovery, dependency revalidation, and compaction, the
minimum failure categories are:

- ImplementationKindMismatch;
- ImplementationArtifactMismatch;
- ImplementationConfigurationSchemaMismatch;
- ImplementationConfigurationContractUnsupported;
- PlatformIntegrationUnsupported;
- ModelAdmissionCapabilityUnsupported;
- ExactModelUnavailable;
- ExactModelMismatch;
- HostAttestationMismatch;
- EffectiveConfigurationMismatch;
- CheckpointIncompatible;
- PinnedDependencyRevoked;
- ContextItemTooLarge;
- CompactionInputTooLarge; and
- CompactionPayloadTooLarge.

Model and compaction failures apply to their admitted capabilities. A runtime
without an exposed model selection does not satisfy them by claiming an empty
model set. These failures never trigger harness or platform fallback. A caller
may correct the configuration, activate another reviewed AgentRevision, or start
a new Session after the unavailable dependency is restored.

The plan becomes immutable at SessionStarted. A different implementation,
model, provider route, input, or dependency requires another Session. A
restart of the same Session must reuse the same plan. Live authorization,
budget, credential rotation, and revocation can still stop or restrict the
Session without rewriting its historical plan.

Store the exact canonical plan bytes once and compute the plan digest over
those bytes. Store the digest beside the bytes, never inside the value it
hashes. Readers verify the bytes before decoding and never re-encode a decoded
plan to recreate the digest.

### 3. Keep verified implementations attached to their platform Session

For the verified execution contract, the Session coordinator and
platform-owned harness form the bidirectional execution boundary. A future
AgentImplementationAdapter translates an external product into that boundary
without changing the core Session contract. Its
version and artifact digest are pinned by the immutable registered extension
version.

The platform Session owns:

- durable Session identity and SessionExecutionPlan;
- the parent-child collaboration graph and external delegation operations;
- authorized tool, delegation, and model dispatch;
- transcript and output recording;
- cancellation intent and terminal outcome; and
- delivery of a child Session result to its waiting parent.

The platform-owned harness runs the loop for a Session. Four durable records
must not be conflated:

- The typed Session event log is the authoritative record of platform facts.
  It alone rebuilds the Session aggregate and every read model.
- An aggregate [snapshot](../glossary/snapshot) is an advisory persisted fold of
  that log. If it is missing or invalid, replay starts earlier without changing
  Session meaning.
- A harness recovery checkpoint is opaque process state used only when the
  platform needs to continue an in-flight loop under the same pinned plan. It
  cannot replace event replay, prove a platform fact, or act as a Session
  snapshot.
- A projection or consumer [checkpoint](../glossary/checkpoint) is only a
  processed stream position and is not any of these state artifacts.

The protobuf named `Checkpoint` retains its wire name, but this ADR calls that
object a harness recovery checkpoint to keep the meanings distinct.

The normative v1 platform harness permits repeated compactions. Each successful
compaction produces one model-visible view consisting of a bounded inline,
self-sufficient summary of an older effective Session-history prefix and a
non-empty, policy-bounded tail containing complete `turn_id` groups. The cut
lies between complete turn groups and never splits one between the summary and
the tail. It need not end on a turn event: an intervening control event may name
the ordinal boundary, and a fork's context-root boundary before its first local
turn is also a valid cut. Keeping recent turns intact preserves context when the
summary omits something and avoids separating a tool request from its result.
The resolved policy requires at least one complete retained turn and the
Session command revalidates that requirement against folded history. Although
the event schema permits a cut at the current head for a future summary-only
policy, the v1 command rejects any cut that leaves no complete retained turn.

When a prior compaction is usable under
[ADR#0035](./0035-session-store-decider-aggregate.md)'s rewind and privacy
rules, its summary and retained tail are inputs to the successor, whose summary
must subsume that usable view and the newly covered prefix. Before generation,
the owning harness compiles the exact effective covered input after rewind
selection, redaction masks, artifact erasure, and fork-prefix resolution. The
covered-input contract version in the immutable plan defines the canonical,
length-delimited representation. The
harness computes `covered_input_digest` over those exact canonical bytes, and
the Session command independently recomputes and compares it before append.
This proves which masked input the candidate covered, not whether natural
language captured every meaning. [ADR#0035](./0035-session-store-decider-aggregate.md)
defines how a later privacy change invalidates a marker whose digest no longer
matches.

Only the owning platform harness may validate a candidate and command its
append to the Session. The command principal must authenticate that harness for
the Session and its current Ready, non-ended ExecutionAttempt. The producer's
plan digest and model role select either the pinned primary ModelSelection or
the exact pinned auxiliary ModelSelection under the typed role `compaction`.
The event's optional model string is provider-reported telemetry only; it may be
absent and does not select or authorize a model. V1 admits neither an arbitrary
model nor another Agent's output as an authoritative summary. A delegated Agent
may provide guidance or work product, but the owning harness treats that result
only as input and cannot append it directly as the summary. A manual compaction
with no Ready owning attempt must start and admit a new attempt or fail without
appending a marker.

The resolved policy must trigger early enough to leave its declared input
safety margin and must define a deterministic maximum number of bounded
compaction passes. Intermediate pass state is process-local input to generation,
not a durable sidecar; only the final self-sufficient marker is authoritative.
If an aggregate covered prefix cannot be processed within those limits, the
harness returns `CompactionInputTooLarge`. This includes a fork whose inherited
prefix already exceeds the compaction input limit before the child has a
complete local turn: v1 cannot replace that indivisible prefix while also
retaining a non-empty child-local turn, so it fails rather than silently
switching policies.

Before model dispatch, the harness represents oversized inline content as
bounded model-visible text plus an ArtifactRef when the content contract
supports that representation. If one indivisible turn still cannot fit the
reserved retained-tail budget, it returns `ContextItemTooLarge`. The Session
command obtains the current NATS server `INFO.max_payload` and subtracts the
exact encoded decider envelope and header bytes plus every non-summary event
field. The remaining bytes are the hard budget for the encoded
`summary_content` field. Guidance and every other variable non-summary value
must already satisfy the resolved policy's individual bounds.

The command returns `CompactionPayloadTooLarge` before append when the fully
encoded append exceeds `INFO.max_payload`, equivalently when the encoded summary
field exceeds its remaining allowance. If only a reducible summary is too
large, the harness may retry with a smaller legal summary. If
the bounded non-summary event, envelope, and header bytes leave no room for the
minimum legal summary, the failure is irreducible and no retry is promised. It
never splits a turn, silently drops content, switches to a summary-only view, or
relaxes the selected policy. These typed outcomes are owned by the harness and
Session coordinator, not the Session Store payload validator. A recoverable
payload rejection appends nothing. If the plan's bounded retries cannot produce
a legal context, the coordinator records `ExecutionAttemptEnded` with
`ATTEMPT_OUTCOME_FAILED`; when no plan-identical attempt can make progress, the
liveness path records `SessionFailed` with
`SESSION_FAILURE_REASON_RESOURCE_EXHAUSTED`. Human-readable detail remains
diagnostic and never controls behavior. V1 creates no separate compaction
artifact; [ADR#0035](./0035-session-store-decider-aggregate.md) owns the
self-sufficient in-stream marker and its fold semantics.

A harness recovery checkpoint is admissible only after capture completes and
the Session command boundary verifies all the following before
`CheckpointProduced` is recorded:

- the sealed state includes every harness-relevant effective Session fact from
  the beginning of the Session through the declared `covers_through` cut, so
  restoring it produces the same harness state as a fresh replay through that
  cut, proven by the capture attestation below rather than assumed;
- the state has been sealed as a durable artifact independently of the process
  memory that produced it;
- independently fetching the sealed bytes and recomputing their digest
  succeeds;
- the producing ExecutionAttempt and immutable SessionExecutionPlan digest
  match the Session;
- the harness can correlate its cut to one settled platform `covers_through`
  ordinal without guessing; and
- `checkpoint_type` identifies a format supported by the implementation version
  committed by the plan.

Semantic coverage and replay equivalence cannot be recomputed from opaque
bytes, so the first verification rests on a capture attestation rather than on
the harness's word. When capture completes, the platform-controlled supervisor
of the producing attempt (section 4) computes an effective-history digest over
the harness-relevant effective Session facts it delivered, in fold order, from
the beginning of the Session through `covers_through`, and signs, under that
attempt's confirmation key, a binding of the artifact reference and digest,
`checkpoint_id`, `checkpoint_type`, producing ExecutionAttempt,
SessionExecutionPlan digest, `covers_through`, and that effective-history
digest. Admission verifies the signature against the producing attempt's
confirmation-key thumbprint, requires every attested value to equal the
corresponding field of the checkpoint evidence being admitted, recomputes the
effective-history digest from authoritative Session history, and requires
equality with the attested value. An attestation is not transferable:
evidence whose artifact reference, digest, id, type, cut, attempt, or plan
digest differs from what the supervisor signed is rejected, never partially
matched.
The admitted checkpoint evidence retains the attestation reference and digest
beside the effective-history digest, so restoration re-verifies the same proof
before trusting any bytes. The attestation proves the binding: the measured
harness and supervisor boundary verified under section 4 captured the sealed
state from exactly the attested effective history. A missing, unverifiable, or
mismatched attestation rejects the checkpoint.

`Checkpoint.covers_through` is the core Session replay cut. Internal harness
coordinates stay inside the opaque, versioned artifact and never become core
Session facts or `SessionOrdinal` values. If any admission proof is
unavailable, that checkpoint cannot continue the in-flight loop. The platform
then replays authoritative Session history and starts a fresh ExecutionAttempt;
only incomplete authoritative history or an indeterminate side effect requiring
reconciliation can block recovery.

Restoration is one guarded workflow:

1. `StartExecutionAttempt` folds the Session at current head `H`, selects an
   eligible admitted checkpoint, and appends `ExecutionAttemptStarted` under
   `At(H)` with that exact checkpoint evidence.
2. The supervisor fetches the sealed artifact again and verifies its digest,
   format, capture attestation, producing attempt, plan, and effective
   `covers_through` cut before trusting any bytes.
3. The harness restores the sealed state, then replays the exact
   harness-relevant effective tail after `covers_through` through `H`, using the
   same rewind, redaction, and compaction interpretation as a fresh replay.
4. Only after the tail reaches `H` may the attempt record Ready or receive new
   work. Facts appended after `H` remain queued for normal delivery.

Eligibility in step 1 demands more than an intact artifact: the covered prefix
must still mean at `H` what it meant when the checkpoint was admitted. Tail
replay applies interpretation only to facts after `covers_through`; it cannot
rebuild the sealed prefix. Any fact folded through `H` that reinterprets
history at or before the cut therefore disqualifies the checkpoint: a rewind
that makes `covers_through` ineffective, a redaction targeting any event at or
before it, or an artifact erasure reaching an artifact recorded at or before
it. Without this rule, a restored attempt would keep content a fresh replay
masks. A compaction marker in the tail does not disqualify, whatever range it
covers: applying it is the loop's ordinary live operation, the restored
attempt and a fresh replay hold the same covered facts and fold the same
self-sufficient marker, and unlike redaction and erasure it removes nothing a
fresh replay would still deliver. An ineligible checkpoint falls back to
fresh replay from authoritative history, which applies the changed
interpretation from the first fact.

This makes checkpoint restore observationally equivalent to rebuilding the
harness from authoritative history through the same selected head. A checkpoint
is an optimization for process-local state, never an alternate history.

Claude, Codex, or another product may be integrated later by translating at
the platform edge. Such an integration cannot add product session ids,
transcript layouts, bridge cursors, or product-specific resume semantics to the
core Session schema. Its external recovery material remains outside this
platform harness contract.

The logical harness exchange is:

| Direction | Operation | Required effect |
| --- | --- | --- |
| Platform to harness | Start | Bind the exact Session, plan bytes, and plan digest. |
| Harness to platform | Ready | Prove the admitted implementation and effective configuration are running. |
| Platform to harness | DeliverInput | Deliver immutable work or continuation input. |
| Harness to platform | Output | Record ordered model-visible output. |
| Harness to platform | ToolRequested | Ask the platform to authorize and dispatch a declared tool. |
| Platform to harness | ToolResult | Return the typed result or denial. |
| Harness to platform | DelegateRequested | Ask the platform to create an authorized child Session or external delegation operation. |
| Platform to harness | DelegateResult | Return the recorded result to the waiting loop. |
| Harness to platform | ModelRequested | Ask the Session model proxy to call one planned model route. |
| Platform to harness | ModelResult | Return the response for the same planned operation. |
| Harness to platform | CheckpointProduced | Record an admitted harness recovery checkpoint. |
| Platform to harness | Cancel | Stop new work and acknowledge cancellation. |
| Harness to platform | Completed or Failed | Record one typed terminal outcome. |

Every exchange binds the Session id and plan digest. Retryable requests carry
a stable operation id and request digest. Ordered output carries a monotonic
sequence, while reconnect behavior stays inside the harness transport and
operation ledger. If continuity cannot be proven, the coordinator restores an
admitted harness recovery checkpoint or replays authoritative history into a
fresh attempt.

The Session keeps a durable operation ledger. Before a tool or delegation side
effect, it reserves the operation id and typed request digest. A retry with the
same id and bytes observes the pending operation or its recorded result.
Reusing the id with different bytes is a typed conflict. Tool dispatch identity
and child Session or external delegation identity are durable before external
dispatch.

Exactly-once external tool execution is never assumed. A tool must honor the
stable dispatch identity or support outcome reconciliation. If a crash leaves
a non-idempotent outcome indeterminate, recovery records ToolOutcomeUnknown
and does not automatically repeat the side effect.

Harness spawning cannot create hidden collaboration state. A spawn must map
one-for-one to either an authorized child Session or an authorized external
delegation operation, then wait for DelegateResult. Otherwise it is disabled.
Each child Session has its own revision, plan, authorization, transcript, and
terminal outcome.

An external delegated agent does not become a child Session. The parent
Session ledger records an ExternalDelegationOperation with the stable operation
id, parent Session and plan digest, resolved delegate reference from the plan,
authenticated remote subject, authorization reference, request digest, status,
correlation id, and response or failure digest. The harness receives only the
resulting DelegateResult. This gives the parent loop a durable return path
without claiming knowledge of the external system's implementation, model,
internal tools, transcript, or execution plan.

The delegation or integration plane owns the external destination binding,
endpoint, and authentication data. SessionExecutionPlan copies only the
resolved non-secret reference and digest required to authorize dispatch. At
dispatch, that plane authenticates the [transport](../glossary/transport) without exposing credential
material to AgentConfiguration, the harness, the prompt, or the
operation payload.

### 4. Verify the platform harness before Ready

Ready is an admission proof, not only a health signal. It binds:

- Session id and plan digest;
- AgentImplementationVersion reference and definition digest;
- measured platform harness identity, version, and artifact digest;
- measured supervisor artifact digest;
- effective harness configuration digest;
- supervisor confirmation-key thumbprint;
- restored continuation evidence when resuming; and
- the authenticated execution identity that produced the evidence.

The Session does not become runnable until the coordinator validates Ready and
confirms that every required model-access grant and live launch authorization
is active.

The effective configuration digest covers the exact non-secret harness
configuration projected from AgentConfiguration and SessionExecutionPlan.
Secrets, temporary credentials, and sender-constrained proof keys are excluded.
The digest must equal the expected value already stored in the plan.

For an in-process or remote platform-harness launch, a platform-controlled
supervisor verifies the exact harness artifact and effective configuration and
produces Ready evidence. A remote launch must attest the deployed harness and
supervisor boundary; attesting only an endpoint or transport driver is
insufficient.

**Future registered edge extensions.** If a later ADR admits an external
product, its extension contract additionally pins and attests the native product
and adapter artifacts. A platform-controlled supervisor must still mediate
Session-bound model requests without exposing a grant token, proof private key,
provider API key, or renewable credential to that product. If the product and
adapter boundary cannot provide this evidence, the system is an external
delegated agent rather than a verified Session implementation.

Hosting is deliberately not a first-class resource in this ADR. Launch attempts
may record placement, process, container, remote endpoint, health, restart, and
teardown facts in Session or deployment records. A host may change between
Sessions, or during a verified restart, only when it runs the same pinned
implementation and plan. If it changes behavior, it must be represented in a
new implementation version or AgentConfiguration.

Each launch or restart creates one Session-owned ExecutionAttempt identity and
an append-only sequence of immutable facts:

    ExecutionAttemptStarted
      execution_attempt_id
      session_id + session_execution_plan_digest
      attempt_number
      previous_attempt_id?
      restored_checkpoint?
        checkpoint_id + reference + checkpoint_type + digest
        implementation_version + producing_execution_attempt_id
        covers_through + session_execution_plan_digest
        capture attestation reference + digest
        effective history digest
      host artifact or driver reference + digest
      authenticated remote subject?
      isolation and placement facts
      started_at

    ExecutionAttemptReady
      execution_attempt_id
      Ready attestation reference + digest
      ready_at

    ExecutionAttemptEnded
      execution_attempt_id
      outcome = failed | cancelled | terminated
      ended_at

ExecutionAttempt facts are evidence about one launch, not reusable
configuration and not a reusable execution-runtime resource. Restart never
edits the prior attempt. It creates a new attempt under the same immutable plan
and records its lineage. Admission rejects a new attempt when its host changes
implementation behavior or cannot reproduce the planned harness artifact and
effective configuration. Cancellation intent belongs to the Session, while
ExecutionAttemptEnded records how that intent affected the attempt.

A continuing attempt records the exact admitted harness recovery checkpoint it
restores. Ready attests that the artifact was verified, its state restored, and
the effective tail replayed through the head selected by the start command. The
prior attempt's model grants are revoked, and the new supervisor creates a fresh
confirmation key. Only after Ready validates may the platform issue new grants
bound to the new ExecutionAttempt and the unchanged resolved routes. If
checkpoint continuity or grant rebinding cannot be proven, that continuation is
rejected and the platform starts a fresh attempt from authoritative history
instead of rewriting the plan.

### 5. Bind one typed settings payload to the exact runtime

The general AgentConfiguration has one `runtime` binding and one
`google.protobuf.Any` settings envelope. Each runtime owns the complete message
inside that envelope, including the platform harness. The platform harness is
the only built-in v1 execution implementation; this does not give its native
settings fields universal meaning for other runtimes.

The immutable AgentImplementationVersion pins the accepted settings [type URL](../glossary/type-url),
descriptor-set digest, settings contract version, validation rules, defaults,
and adapter artifacts. Admission checks the runtime/settings pairing and
validates the complete payload. The type URL identifies a registered contract;
it never authorizes fetching code or schemas from the network. Mismatched,
unavailable, undecodable, or unsupported settings fail closed.

One envelope is sufficient because the runtime's complete settings message may
contain its own nested lists or unions. A general `repeated Any` would add
ordering, multiplicity, overlap, and merge rules without an owner. A runtime
that needs composition defines those rules in its own settings contract.

Platform-owned skill pins, memory/tool/delegate declarations, and caller
variables remain typed common declarations under
[ADR#0025](./0025-agent-definition-data-ownership.md) and
[ADR#0062](./0062-runtime-owned-settings-and-platform-declarations.md). Every
declared resource integration requires support in the pinned adapter. An absent
declaration requires no such integration capability. These fields never carry
skill or memory contents, policy, live grants, or credentials. Common description
and selectable labels remain platform-owned metadata; they do not require
native runtime consumption.

A registered product integration pins its native product and adapter artifacts
inside its immutable implementation definition. Its settings cannot contain
provider credentials, secret values, or live grants. Runtime ownership does not
move another platform resource's authority into the settings envelope.

The managed-model Session plan retains typed ModelSelection and
ResolvedModelRoute values for the capability it admitted. They are derived
values with verified provenance to the pinned settings, not authored sibling
settings on AgentConfiguration. StoredSessionExecutionPlan retains exact plan
bytes and a separate digest as required by section 2.

### 6. Fail closed across protobuf version skew

Knowing a settings type is insufficient when a reader cannot interpret a newer
contract version of its payload. With unknown_fields=false, it can silently
drop additive fields when rewriting and could ignore behavior required by the
newer configuration.

Apply these rules:

1. Every boundary validates the exact runtime reference and its accepted
   settings type. A missing or mismatched binding is unsupported, never a
   default implementation.
2. Every runtime version pins an explicit settings contract version. Writers,
   admission services, harnesses, supervisors, and adapter handlers advertise
   the exact versions they can interpret.
3. Admission requires support for the type, contract version, and every declared
   platform integration. A newer behavior-affecting settings field requires a
   newer contract version even when protobuf considers it additive.
4. SessionExecutionPlan and ResolvedModelRoute carry independent contract
   versions. Every consumer must support the plan and capability values it
   interprets. Consumers of admitted model routes must support their route
   contract. A missing or unsupported version is a typed failure, never a
   default. Behavior-affecting or security-affecting fields require a newer
   contract version even when protobuf considers them additive.
5. A component may relay exact immutable bytes it does not interpret, but it
   may not decode, modify, and rewrite a record whose settings, plan, or
   admitted capability contract it does not fully support.
6. New runtime settings contracts and platform capabilities require coordinated
   rollout and compatibility tests before configurations using them are
   admitted. A breaking settings schema uses a new type URL and immutable
   runtime version.
7. Digests cover the exact admitted bytes. Verification never depends on
   decoding and re-encoding with a possibly older schema.

ProtoJSON is diagnostic or interoperable output, not the canonical persistence
form. Unknown JSON keys are not an extension mechanism.

Durable [event envelopes](../glossary/event-envelope) store the stable full name and exact bytes of each
concrete [event](../glossary/event). SessionStarted stores StoredSessionExecutionPlan once in the
Session stream. Do not persist the bytes of a top-level event oneof wrapper.
Large immutable implementation definitions and harness recovery
checkpoints may be external only when the event retains their exact reference,
type, and digest.

### 7. Apply the model to concrete products

The platform-managed loop is the only normative v1 execution implementation.
Product rows describe configuration boundaries for possible future integrations,
not a promise that an SDK supports the required inspection or execution
capabilities.

| Arrangement | Runtime-owned settings | Platform-managed model capability | Execution status |
| --- | --- | --- | --- |
| Platform managed loop | Its own typed settings, including native model and compaction controls | Derives and enforces exact model selections | Normative built-in v1 implementation |
| Codex or Claude Code adapter | A distinct complete settings contract for each pinned adapter; no assumed common native fields | Requires adapter-specific proof of verifiable selection and route enforcement | Future edge integration research |
| Composite or externally managed product | Its own settings and immutable implementation binding | Hidden or dynamic model control cannot claim exact managed-model admission | Verified integration only if all required capabilities are proven; otherwise external delegation |

Product composition, hidden spawning, native fallback, and product-specific
resume behavior cannot become core Session semantics through the settings
envelope. An integration either translates them into supported platform
commands or keeps them outside the verified Session boundary. A valid Agent
configuration does not establish support for every execution mode.

### 8. Bound mutability explicitly

| Record or state | Mutable? | Change mechanism |
| --- | --- | --- |
| Agent identity | No | Create another Agent |
| AgentConfiguration | No | Create a new configuration |
| AgentRevision | No | Activate a new revision |
| Implementation kind, version, or options | No within a revision | Proposal and new revision |
| Runtime model settings and derived managed-model selections | No within a revision | Proposal and new revision |
| AgentImplementationVersion definition | No | Publish another version |
| SessionExecutionPlan | No | Start another Session |
| Implementation availability or revocation | Yes | Live policy with no fallback |
| Provider credential behind a stable SecretRef | Yes | Rotate or revoke under [ADR#0023](./0023-secret-management-and-key-custody-direction.md) |
| Authorization, grants, and budgets | Yes | Evaluate live without changing pins |
| Launch and hosting lifecycle | Yes | Session or deployment records |

Revoking a pinned implementation may prevent new Sessions or terminate an
existing Session under policy. It never upgrades that revision to another
implementation. Credential rotation may preserve a stable CredentialBinding
only under the continuity rules in
[ADR#0032](./0032-model-route-and-credential-binding.md). It never changes the
selected model.

## Alternatives Rejected

### One AgentRuntime resource

A single resource for loop behavior, product version, model choice, hosting,
and credentials obscures which changes require a new AgentRevision. Product
documentation does not use runtime consistently enough to make the term a
safe domain boundary.

### Stable implementation family with Session-selected latest version

An immutable version record does not make an AgentRevision immutable when a
later Session can select a newer version. The exact version belongs in
AgentConfiguration.

### Admit hidden model choice to the managed-model path

A runtime may own model selection without exposing it to the platform. That
settings contract can be valid, but it cannot prove the exact selections needed
for managed route admission. Native defaults are admissible for that path only
when the pinned contract fixes them and the adapter derives and enforces the
resulting selection. Hidden control is not evidence of a model-free runtime.

### Model hosting as a first-class resource now

The current decision only requires exact implementation artifacts and
auditable launch facts. A reusable hosting resource would add policy and
versioning before a proven domain invariant requires it.

### Give the platform harness's settings a universal schema

A mandatory common model, prompt, or compaction shape would require unrelated
runtimes to accept controls they may not expose. One `Any` preserves each
runtime's typed message while the exact version binding prevents mismatched
payloads. This differs from an unrestricted map or an unvalidated envelope:
registration, full validation, and capability checks remain mandatory.

### Treat dynamic OpenClaw as an ordinary verified implementation

OpenClaw can select components, models, plugins, and delegates dynamically.
It is verified only when every behaviorally relevant choice is pinned and
attested. Otherwise it is treated as an external delegated agent.

## Consequences

- Every AgentRevision answers exactly which runtime version, settings type,
  admitted settings bytes, and platform declarations it binds. Exact managed
  model selections are available only through an admitted adapter capability.
- Runtime upgrades and settings changes are reviewed, versioned Agent changes.
  Managed-model admission cannot replace their derived model selections.
- Every Session records the exact implementation, configuration projection,
  dependencies, admitted capabilities, and canonical plan bytes. Model routes
  appear only for the managed-model capability.
- Session events, aggregate snapshots, harness recovery checkpoints,
  and read-side checkpoints have separate authority and failure behavior.
- Provider credentials remain outside Agent and implementation configuration.
- A future registered edge extension owns its product and adapter attestation
  inside the extension contract; it does not add product fields to the built-in
  harness plan.
- The platform-managed harness loop provides the normative v1 implementation
  model. Codex, Claude Code, and OpenClaw remain future edge compatibility
  cases and cannot shape the core Session contract.
- The platform avoids premature hosting resources while preserving launch
  evidence in Session and deployment records.
- Protobuf evolution requires explicit configuration contract capability
  gating in addition to ordinary wire compatibility.

## References

- [ADR#0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [ADR#0023: Secret Management and Key Custody Direction](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0024: Agent Platform Stream Topology](./0024-agent-platform-stream-topology.md)
- [ADR#0025: Agent Definition Data Ownership](./0025-agent-definition-data-ownership.md)
- [ADR#0032: Model Route and Credential Binding](./0032-model-route-and-credential-binding.md)
- [ADR#0062: Runtime-Owned Settings and Platform Declarations](./0062-runtime-owned-settings-and-platform-declarations.md)
- [Agent platform decision record](../research/agent-platform/decision-record.md)
- [Codex App Server](https://developers.openai.com/codex/app-server)
- [Codex custom model providers](https://learn.chatgpt.com/docs/config-file/config-advanced#custom-model-providers)
- [Claude Agent SDK](https://platform.claude.com/docs/en/agent-sdk/overview)
- [Claude Code Agent SDK product dossier](../research/agent-platform/products/claude-code-agent-sdk.md)
- [OpenClaw product dossier](../research/agent-platform/products/openclaw.md)
- [OpenClaw agent runtimes](https://docs.openclaw.ai/concepts/agent-runtimes)
