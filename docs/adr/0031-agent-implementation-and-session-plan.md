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
2. Which models does that implementation use?

Terms such as runtime and harness have different meanings across products.
Using either term as the main platform abstraction made the answer harder to
see. It also allowed a supposedly immutable Agent revision to acquire a newer
implementation or a different model when a Session started.

This decision uses a smaller model:

    Agent
      stable identity

    AgentConfiguration
      exact implementation version and typed configuration
      exact primary and auxiliary model selections
      the rest of the Agent's behavior

    AgentRevision
      immutable numbered binding to one AgentConfiguration

    SessionExecutionPlan
      immutable record of the exact revision, implementation, models,
      provider routes, and dependencies admitted for one Session

Codex and Claude Code are implementations in this model. An OpenAI or Claude
model is a separate model selection. A process, container, microVM, or remote
service may host an implementation, but hosting is not itself an Agent
implementation.

OpenClaw uses its own precise product vocabulary: an agent runtime owns a
prepared model loop, and a harness implements that runtime. The platform does
not copy those nouns into its core model. For a verified configuration, an
OpenClaw adapter maps the exact behavioral component to
AgentImplementationVersion and preserves OpenClaw's native terms inside typed
OpenClaw configuration.

This ADR refines the ownership described by draft
[ADR#0025](./0025-agent-definition-data-ownership.md). The exact
implementation belongs to AgentConfiguration, not to the stable Agent
identity. Changing implementation kind, version, configuration, or model is a
reviewed behavior change that produces a new AgentRevision.

It also refines the frozen
[agent-platform decision record](../research/agent-platform/decision-record.md),
which used `runtime` as an immutable Agent field and allowed a Session to
override the model. That record remains historical research input. If this ADR
is accepted, its exact implementation and model pins become the normative
decision.

The repository already establishes adjacent constraints:

- [ADR#0009](./0009-protocol-buffers-wire-contracts.md) requires typed,
  versioned protobuf contracts and prefers explicit messages over untyped
  maps.
- [ADR#0023](./0023-secret-management-and-key-custody-direction.md) keeps
  secret values behind the secrets service.
- [ADR#0024](./0024-agent-platform-stream-topology.md) places a durable fact
  in the stream whose ordering is required for its invariant.
- Draft [ADR#0032](./0032-model-route-and-credential-binding.md) separates
  exact model selection from provider connections and credentials.
- The generated protobuf types use unknown_fields=false. Older readers can
  discard unknown arms and unknown fields, so version skew must fail closed.

This ADR defines logical ownership, Session admission, the implementation
adapter, remote verification, and protobuf modeling. It does not select a
container orchestrator, define a general hosting resource, or move provider
credentials into Agent configuration.

## Decision

### 1. Pin implementation and models in AgentConfiguration

Use these normative logical records:

    AgentImplementationVersion
      implementation_version_id
      built-in kind or registered extension identity
      native product version
      adapter artifact reference and digest
      native artifact references and digests
      adapter contract version
      configuration contract version
      supported model protocols and capabilities
      definition_digest

    AgentImplementation
      exact AgentImplementationVersion reference and definition digest
      typed per-Agent implementation configuration and digest

    ModelSelection
      exact ModelVersion reference and definition digest
      deterministic model parameters

    AgentConfiguration
      agent_implementation -> AgentImplementation
      primary_model_selection -> ModelSelection
      auxiliary_model_selections -> typed role + ModelSelection
      instructions, wrappers, variables, skills, dependencies, and labels
      configuration_digest

    AgentRevision
      agent_id
      revision_number
      configuration_ref
      configuration_digest
      source

AgentImplementationVersion describes a reusable immutable release. Its
definition digest commits to the product and adapter artifacts, native product
version, supported contracts, and implementation capabilities. A mutable tag
such as latest is not an exact version.

Every behavior-significant artifact is pinned transitively. This includes a
bundled CLI or SDK, plugins, nested components, composition rules, and generated
assets when they can affect implementation behavior. Recording only a wrapper
or launcher version is insufficient.

AgentImplementation inside AgentConfiguration selects one exact version and
contains its typed per-Agent options. The AgentConfiguration digest commits to
that selection, those options, every model selection, and the rest of the
Agent behavior. AgentRevision binds the same configuration and digest after
activation.

ModelSelection identifies an exact versioned model catalog record, not a
display name, mutable provider alias, auto value, or provider credential. Its
parameters are part of AgentConfiguration. The implementation cannot replace
that model with a native default or fallback.

An implementation-required auxiliary model is also explicit in
AgentConfiguration under a typed role. An implementation release cannot add a
hidden model call. Adding a role or changing any model produces a new
AgentConfiguration and AgentRevision.

The stable Agent record contains no implementation family. Changing from
Codex to Claude Code is a Proposal and a new AgentRevision. Governance may
require a sibling Agent for a particular change, but the storage model does
not silently create that rule.

For example, AgentRevision 7 can pin `codex-release-42` plus
`model-release-17`. A Session may resolve that exact model through an admitted
Bedrock connection and credential binding, but it cannot replace either pin.
Moving the same plan to another conforming host does not create a revision.
Selecting Claude Code, a newer Codex release, or another model requires a new
AgentConfiguration and AgentRevision.

The following boundary decides where a value belongs:

- If the Agent author or implementation controls a value that can change
  prompts, context, tool or delegate sequencing, stopping behavior, model
  choice, or model-visible state, it belongs in the immutable implementation
  version or AgentConfiguration.
- If a value adapts an already-pinned ModelSelection to an external provider,
  it belongs to the exact ResolvedModelRoute in the immutable
  SessionExecutionPlan. Because provider translation can affect observable
  behavior, verification evidence is scoped to that route and changing it
  requires a new Session.
- If it only places, isolates, starts, monitors, or stops the exact pinned
  implementation without changing those semantics, it is deployment or
  Session launch state.

An OCI image may therefore be an implementation artifact when it is the
immutable distribution of the pinned product. The cluster, node, process
supervisor, filesystem allocation, and network placement that run it remain
hosting details.

### 2. Bind one immutable SessionExecutionPlan

Before a Session becomes runnable, create one SessionExecutionPlan:

    SessionExecutionPlan
      session_id
      agent_revision_ref
      agent_configuration_ref + configuration_digest
      agent_implementation_version_ref + definition_digest
      implementation_configuration_digest
      effective_native_configuration_digest
      primary_model_selection
      primary_resolved_model_route
      auxiliary_model_selections + resolved routes
      resolved variable bindings
      resolved tool, delegate, memory, and skill versions
      work contract and input references
      adapter contract version
      session-plan contract version
      resolved-model-route contract version

The implementation and model selections are copied from the pinned
AgentConfiguration. Session admission verifies them but never chooses a
different implementation, implementation version, model, or deterministic
parameter. V1 has no Session override for those values.

The resolved model routes add the provider connection, non-secret credential
binding reference, exact provider driver, and protocol required to serve each
ModelSelection. They cannot substitute another model. Attempt-scoped
model-access grants, secret values, and short-lived provider credentials never
enter the plan.

Admission proceeds in this order:

1. Load the requested AgentRevision and verify AgentConfiguration bytes and
   digest.
2. Load the exact AgentImplementationVersion and verify its definition,
   native product artifacts, and adapter artifact digests.
3. Validate the typed implementation configuration against the exact
   configuration contract version.
4. Read the exact primary and auxiliary ModelSelection values from
   AgentConfiguration.
5. Resolve an authorized provider route and CredentialBinding for each exact
   model under [ADR#0032](./0032-model-route-and-credential-binding.md).
6. Resolve tools, delegates, memories, variables, work input, and other
   Session dependencies.
7. Verify that the implementation supports every model protocol and required
   Session capability.
8. Build the exact native configuration projection and its expected digest.
9. Store the canonical SessionExecutionPlan bytes and digest atomically with
   SessionStarted.
10. Authorize the first launch only after observing that durable start fact.
    Create model-access grants only after Ready binds them to a specific
    ExecutionAttempt.

A missing, ambiguous, unavailable, unauthorized, revoked, incompatible, or
digest-mismatched value rejects Session start with a typed failure. Admission
never falls back to another implementation release, model, provider account,
credential, or extension.

Across admission, Ready, recovery, and dependency revalidation, the minimum
failure categories are:

- ImplementationKindMismatch;
- ImplementationArtifactMismatch;
- ImplementationConfigurationSchemaMismatch;
- ImplementationConfigurationContractUnsupported;
- ExactModelUnavailable;
- ExactModelMismatch;
- HostAttestationMismatch;
- EffectiveConfigurationMismatch;
- CheckpointIncompatible; and
- PinnedDependencyRevoked.

These failures never trigger native or platform fallback. A caller may correct
the configuration, activate another reviewed AgentRevision, or start a new
Session after the unavailable dependency is restored.

The plan becomes immutable at SessionStarted. A different implementation,
model, provider route, input, or dependency requires another Session. A
restart of the same Session must reuse the same plan. Live authorization,
budget, credential rotation, and revocation can still stop or restrict the
Session without rewriting its historical plan.

Store the exact canonical plan bytes once and compute the plan digest over
those bytes. Store the digest beside the bytes, never inside the value it
hashes. Readers verify the bytes before decoding and never re-encode a decoded
plan to recreate the digest.

### 3. Keep every implementation attached to its platform Session

An AgentImplementationAdapter is the bidirectional boundary between the
platform Session and one native implementation. Its version and artifact
digest are pinned by AgentImplementationVersion.

The platform Session owns:

- durable Session identity and SessionExecutionPlan;
- the parent-child collaboration graph and external delegation operations;
- authorized tool, delegation, and model dispatch;
- transcript and output recording;
- cancellation intent and terminal outcome; and
- delivery of a child Session result to its waiting parent.

The native implementation owns its private loop state while it runs. A durable
checkpoint is an opaque typed artifact or reference whose schema, digest, and
implementation version are recorded by the Session.

The logical adapter exchange is:

| Direction | Operation | Required effect |
| --- | --- | --- |
| Platform to adapter | Start | Bind the exact Session, plan bytes, and plan digest. |
| Adapter to platform | Ready | Prove the admitted implementation and effective configuration are running. |
| Platform to adapter | DeliverInput | Deliver immutable work or continuation input. |
| Adapter to platform | Output | Record ordered model-visible output. |
| Adapter to platform | ToolRequested | Ask the platform to authorize and dispatch a declared tool. |
| Platform to adapter | ToolResult | Return the typed result or denial. |
| Adapter to platform | DelegateRequested | Ask the platform to create an authorized child Session or external delegation operation. |
| Platform to adapter | DelegateResult | Return the recorded result to the waiting loop. |
| Adapter to platform | ModelRequested | Ask the Session model proxy to call one planned model route. |
| Platform to adapter | ModelResult | Return the response for the same planned operation. |
| Adapter to platform | CheckpointProduced | Record an opaque checkpoint reference and digest. |
| Platform to adapter | Cancel | Stop new work and acknowledge cancellation. |
| Adapter to platform | Completed or Failed | Record one typed terminal outcome. |

Every exchange binds the Session id and plan digest. Retryable requests carry
a stable operation id and request digest. Ordered output carries a monotonic
sequence or equivalent acknowledged cursor. Reconnect resumes from the last
acknowledged position. If continuity cannot be proven, the coordinator restores
an admitted checkpoint or fails the Session.

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

Native spawning cannot create hidden collaboration state. A spawn must map
one-for-one to either an authorized child Session or an authorized external
delegation operation, then wait for DelegateResult. Otherwise it is disabled.
Each child Session has its own revision, plan, authorization, transcript, and
terminal outcome.

An external delegated agent does not become a child Session. The parent
Session ledger records an ExternalDelegationOperation with the stable operation
id, parent Session and plan digest, resolved delegate reference from the plan,
authenticated remote subject, authorization reference, request digest, status,
correlation id, and response or failure digest. The adapter receives only the
resulting DelegateResult. This gives the parent loop a durable return path
without claiming knowledge of the external system's implementation, model,
internal tools, transcript, or execution plan.

The delegation or integration plane owns the external destination binding,
endpoint, and authentication data. SessionExecutionPlan copies only the
resolved non-secret reference and digest required to authorize dispatch. At
dispatch, that plane authenticates the transport without exposing credential
material to AgentConfiguration, the native implementation, the prompt, or the
operation payload.

### 4. Verify local and remote implementations before Ready

Ready is an admission proof, not only a health signal. It binds:

- Session id and plan digest;
- AgentImplementationVersion reference and definition digest;
- measured native product identity, version, and artifact digest;
- measured adapter or supervisor artifact digest;
- effective native configuration digest;
- supervisor confirmation-key thumbprint;
- restored continuation evidence when resuming; and
- the authenticated execution identity that produced the evidence.

The Session does not become runnable until the coordinator validates Ready and
confirms that every required model-access grant and live launch authorization
is active.

The effective configuration digest covers the exact non-secret native
configuration projected from AgentConfiguration and SessionExecutionPlan.
Secrets, temporary credentials, and sender-constrained proof keys are excluded.
The digest must equal the expected value already stored in the plan.

For an in-process or platform-managed launch, a platform-controlled supervisor
verifies artifacts and produces Ready evidence. For a remote pinned
implementation, Ready additionally requires attestation that the native
product build and effective configuration actually deployed at the remote
boundary match the plan. Attesting only the local adapter, remote endpoint, or
transport driver is insufficient.

V1 verified remote model access requires a platform-controlled attested
supervisor beside the remote implementation. The native product sends a
Session-bound ModelRequested operation to that supervisor over an authenticated
local or private channel. The supervisor verifies the Session, plan, route,
operation id, and request digest, then presents the sender-constrained
ModelAccessGrant to the platform model proxy. The native product never receives
the grant token, proof private key, provider API key, or renewable credential.

If that supervisor and attestation boundary cannot be established, the remote
system is treated as an external delegated agent. This is a boundary
classification, not another platform resource type. The platform may authorize
a delegation request and record its result, but it does not claim that the
remote internal implementation, configuration, model calls, or tool calls
satisfy this SessionExecutionPlan.

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
        reference + type + digest + implementation_version
      resume_cursor?
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
implementation behavior or cannot reproduce the planned native artifact and
effective configuration. Cancellation intent belongs to the Session, while
ExecutionAttemptEnded records how that intent affected the attempt.

A continuing attempt records the exact admitted checkpoint and acknowledged
cursor it restores. Ready attests that restoration. The prior attempt's model
grants are revoked, and the new supervisor creates a fresh confirmation key.
Only after Ready validates may the platform issue new grants bound to the new
ExecutionAttempt and the unchanged resolved routes. If checkpoint continuity,
cursor continuity, or grant rebinding cannot be proven, the Session fails
instead of replaying work or rewriting its plan.

### 5. Use typed protobuf unions and registered extensions

Built-in implementations use explicit oneof arms. The oneof case is the
implementation kind. Do not add an enum discriminator beside generic
configuration because the two values could disagree.

The following shapes are illustrative. Concrete packages, field names, and
supporting value objects require their own Buf-validated contract design.

    message AgentImplementation {
      oneof implementation {
        CodexImplementation codex = 1;
        ClaudeCodeImplementation claude_code = 2;
        ManagedImplementation managed = 3;
        OpenClawImplementation openclaw = 4;
        CompositeImplementation composite = 5;
        RegisteredImplementation registered_extension = 100;
      }
      Digest definition_digest = 101;
      Digest configuration_digest = 102;
    }

    message CodexImplementation {
      AgentImplementationVersionRef version = 1;
      ConfigurationContractVersion configuration_contract = 2;
      CodexConfiguration configuration = 3;
    }

    message ClaudeCodeImplementation {
      AgentImplementationVersionRef version = 1;
      ConfigurationContractVersion configuration_contract = 2;
      ClaudeCodeConfiguration configuration = 3;
    }

    message RegisteredImplementation {
      AgentImplementationExtensionVersionRef extension_version = 1;
      google.protobuf.Any configuration = 2;
    }

    message SessionExecutionPlan {
      SessionId session_id = 1;
      AgentRevisionRef agent_revision = 2;
      AgentConfigurationRef agent_configuration = 3;
      AgentImplementationVersionRef implementation_version = 4;
      Digest implementation_definition_digest = 5;
      Digest implementation_configuration_digest = 6;
      Digest effective_native_configuration_digest = 7;
      ModelSelection primary_model_selection = 8;
      ResolvedModelRoute primary_model_route = 9;
      repeated AuxiliaryModelRoute auxiliary_models = 10;
      SessionDependencies dependencies = 11;
      AdapterContractVersion adapter_contract = 12;
      SessionPlanContractVersion plan_contract = 13;
      ModelRouteContractVersion model_route_contract = 14;
    }

    message StoredSessionExecutionPlan {
      bytes plan_bytes = 1;
      Digest plan_digest = 2;
    }

The kind recorded by the selected AgentImplementationVersion must agree with
the AgentImplementation oneof arm. A Codex version in a ClaudeCodeImplementation
arm, a standalone OpenClaw version in a CompositeImplementation arm, or any
other mismatch is ImplementationKindMismatch.

The registered_extension arm is the only place this decision permits
google.protobuf.Any. Its immutable extension version pins one allowed type URL,
descriptor-set digest, adapter artifact digest, configuration contract version,
and capabilities. The platform never fetches code or schemas from the type
URL. Mismatched, unavailable, undecodable, or unregistered values fail closed.
Extension configuration cannot contain provider credentials, Secret values,
live grants, or behavior that belongs in another AgentConfiguration field.

### 6. Fail closed across protobuf version skew

Unknown oneof arms are not the only risk. A reader may know the arm but not
newer additive fields inside that implementation message. With
unknown_fields=false, it can silently drop those fields when rewriting and
could ignore behavior required by the newer configuration.

Apply these rules:

1. Every boundary validates that exactly one known implementation arm exists.
   A missing arm is unsupported, never a default.
2. Every built-in arm carries an explicit configuration contract version.
   Writers, admission services, adapters, and supervisors advertise the exact
   contract versions they can interpret.
3. Admission requires support for the selected arm and configuration contract
   version. A newer behavior-affecting field requires a newer contract version,
   even when protobuf considers the field additive.
4. SessionExecutionPlan and ResolvedModelRoute carry independent contract
   versions. Coordinators, adapters, supervisors, and the model access service
   advertise the exact versions they interpret. Admission requires every plan
   consumer to support both versions. A missing or unsupported version is a
   typed admission failure, never a default. Any behavior-affecting or
   security-affecting field, including a provider driver pin, requires a newer
   contract version even when its protobuf field is additive.
5. A component may relay exact immutable bytes it does not interpret, but it
   may not decode, modify, and rewrite a record whose arm or configuration
   contract, plan contract, or route contract it does not fully support.
6. New built-in arms and contract versions require coordinated rollout and
   compatibility tests before records using them are admitted.
7. Registered extensions use the already-known registered_extension arm. A
   breaking schema uses a new type URL and immutable extension version.
8. Digests cover the exact admitted bytes. Verification never depends on
   decoding and re-encoding with a possibly older schema.

ProtoJSON is diagnostic or interoperable output, not the canonical persistence
form. Unknown JSON keys are not an extension mechanism.

Durable event envelopes store the stable full name and exact bytes of each
concrete event. SessionStarted stores StoredSessionExecutionPlan once in the
Session stream. Do not persist the bytes of a top-level event oneof wrapper.
Large immutable implementation definitions and checkpoints may be external
only when the event retains their exact reference, type, and digest.

### 7. Apply the model to concrete products

| Arrangement | AgentConfiguration implementation | Model ownership | Classification |
| --- | --- | --- | --- |
| Codex | Exact Codex product and adapter version, artifacts, and typed options | Exact compatible model selections are separate AgentConfiguration fields | Verified implementation |
| Claude Code | Exact Claude Code product and adapter version, artifacts, and typed options | Exact compatible model selections are separate AgentConfiguration fields | Verified implementation |
| Platform managed loop | Exact managed implementation version and typed options | Exact model selections remain in AgentConfiguration | Verified implementation |
| Fully pinned OpenClaw | OpenClawImplementation pins the exact build, plugins, effective configuration, and allowed delegation behavior | Every model role is explicit in AgentConfiguration; native auto selection and failover are disabled | Verified standalone implementation only with Ready attestation |
| OpenClaw transparently hosting Codex | Codex is the implementation; OpenClaw is an attested launch fact | Codex uses the exact AgentConfiguration model selections | Verified only when OpenClaw cannot alter loop semantics |
| OpenClaw delegating to Codex | OpenClaw parent Session and Codex child Session | Each AgentConfiguration owns exact model selections and its revision binds them | Two verified Sessions |
| OpenClaw plus Codex layered loop | CompositeImplementation pins every component version and composition rule | Every model role is explicit and pinned | Verified composite implementation |
| Unpinned or auto-selecting OpenClaw | None | Internal model and component choices are opaque | External delegated agent only |

OpenClaw may still make dynamic decisions while running pinned code and
configuration. Dynamic output is not configuration mutability. Dynamically
substituting an unpinned implementation, plugin, model, or hidden child agent
is prohibited for a verified Session.

When evidence cannot prove that OpenClaw is a transparent host, use the
composite classification or treat it as an external delegated agent. Do not
infer transparency from product naming or transport protocol.

### 8. Bound mutability explicitly

| Record or state | Mutable? | Change mechanism |
| --- | --- | --- |
| Agent identity | No | Create another Agent |
| AgentConfiguration | No | Create a new configuration |
| AgentRevision | No | Activate a new revision |
| Implementation kind, version, or options | No within a revision | Proposal and new revision |
| Model selections or deterministic parameters | No within a revision | Proposal and new revision |
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

### Let Codex, Claude Code, or OpenClaw choose the model

Native defaults and fallback make the model invisible to revision review.
Every primary and auxiliary model is explicit AgentConfiguration content.

### Model hosting as a first-class resource now

The current decision only requires exact implementation artifacts and
auditable launch facts. A reusable hosting resource would add policy and
versioning before a proven domain invariant requires it.

### Generic configuration for built-ins

An enum plus map, Struct, bytes, or unrestricted Any loses type safety and
allows the discriminator to disagree with the value. Built-ins use typed
oneof arms. Any is reserved for registered extensions.

### Treat dynamic OpenClaw as an ordinary verified implementation

OpenClaw can select components, models, plugins, and delegates dynamically.
It is verified only when every behaviorally relevant choice is pinned and
attested. Otherwise it is treated as an external delegated agent.

## Consequences

- Every AgentRevision answers exactly which implementation version and models
  it declares.
- Upgrades and model changes are reviewed, versioned Agent changes rather than
  hidden Session resolution.
- Every Session records the exact implementation, configuration projection,
  model routes, dependencies, and canonical plan bytes it admitted.
- Provider credentials remain outside Agent and implementation configuration.
- Local and remote implementations share one adapter contract, but remote
  verified execution requires stronger native artifact, configuration, and
  supervisor attestation.
- Codex and Claude Code provide the primary implementation model. OpenClaw
  remains supported through explicit pinned, composite, delegated, or external
  classifications.
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
- [Agent platform decision record](../research/agent-platform/decision-record.md)
- [Codex App Server](https://developers.openai.com/codex/app-server)
- [Codex custom model providers](https://learn.chatgpt.com/docs/config-file/config-advanced#custom-model-providers)
- [Claude Agent SDK](https://platform.claude.com/docs/en/agent-sdk/overview)
- [Claude Code Agent SDK product dossier](../research/agent-platform/products/claude-code-agent-sdk.md)
- [OpenClaw product dossier](../research/agent-platform/products/openclaw.md)
- [OpenClaw agent runtimes](https://docs.openclaw.ai/concepts/agent-runtimes)
