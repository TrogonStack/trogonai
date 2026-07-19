---
number: "0032"
slug: model-route-and-credential-binding
status: draft
date: 2026-07-18
---

# ADR#0032: Model Route and Credential Binding

## Context

An AgentImplementation executes the exact model selections in an
AgentConfiguration, but the implementation is not the model provider and must
not become the custodian of provider credentials. When their pinned releases
and adapters declare compatibility, Codex, Claude Code, a fully pinned OpenClaw
implementation, and the platform managed loop can use a selected model through
Amazon Bedrock, OpenAI, or another provider. The provider may authenticate with
a tenant API key, delegated OAuth, mutual TLS, an auth-free deployment-local
endpoint, or deployment-attested workload identity.

Those choices have different ownership and lifecycle:

1. The immutable AgentConfiguration owns the exact primary and auxiliary
   ModelSelection values and deterministic parameters.
2. The Session resolves one provider route for each pinned selection. It does
   not choose or override the model.
3. A model provider connection identifies the provider security and billing
   boundary.
4. A credential binding authorizes the platform model access service to use
   that connection.

Calling all of these choices a Codex key, an OpenClaw profile, or implementation
configuration would make provider selection, secret custody, rotation, and
audit behavior depend on whichever implementation happens to run the Session.
Implementation-native defaults and fallback behavior can also substitute a
provider, model, credential, or implementation after Session admission.

The repository already fixes adjacent boundaries:

- [ADR#0007](./0007-configuration-sources.md) requires typed configuration and
  prohibits secret values in configuration files.
- [ADR#0017](./0017-aauth-agent-authentication.md) defines agent identity.
  Agent identity is not a model provider credential.
- [ADR#0023](./0023-secret-management-and-key-custody-direction.md) places API
  credentials and durable OAuth material behind SecretStore, exposes only
  opaque SecretRefs, makes the platform secrets service the only OpenBao
  client, and requires fail-closed resolution with bounded plaintext
  lifetimes.
- Draft [ADR#0025](./0025-agent-definition-data-ownership.md) makes exact
  implementation and model selections immutable AgentConfiguration content
  while keeping live credential bindings outside Agent revisions.
- Draft
  [ADR#0031](./0031-agent-implementation-and-session-plan.md) defines
  SessionExecutionPlan as the immutable Session record of the exact revision,
  implementation, models, provider routes, and dependencies admitted to run.

The current ACP provider operations are not a safe place to define this
boundary. Their authorization headers are arbitrary data, and the global
provider selection method does not identify the platform Session whose grant
would authorize the request.

This ADR defines model provider routing and credential custody for hosted
model access. It does not define implementation or model selection, execution
placement, execution-attempt attestation, host operational authentication,
model catalogs, pricing, budgets, channel credentials, tool credentials, or
detailed provider translation semantics beyond requiring an exact driver and
translation-contract pin.
[ADR#0031](./0031-agent-implementation-and-session-plan.md) owns the exact
implementation and model pins, Session planning, and verified execution
attempts. A credential used only to launch a host or authenticate an execution
supervisor is never a model provider CredentialBinding.

This ADR depends on the SessionExecutionPlan boundary in
[ADR#0031](./0031-agent-implementation-and-session-plan.md) and the
SecretStore boundary in
[ADR#0023](./0023-secret-management-and-key-custody-direction.md). It must not
advance to accepted before those dependencies are accepted with compatible
boundaries.

## Decision

### 1. Keep the resolved route separate from control-plane resources

Use the following logical records and Session-owned values. Their names are
normative vocabulary. Wire spelling and physical persistence remain contract
design work.

    ModelProviderDriverVersion
      driver_version_id
      provider + protocol
      driver artifact reference + digest
      translation contract version
      supported capabilities
      definition_digest
      lifecycle_state

    ModelProviderConnection
      connection_id
      parent -> Hierarchy node
      provider
      endpoint
      provider_subject_fingerprint?
      allowed_models
      supported_protocols
      configuration_digest
      lifecycle_state

    CredentialBinding
      binding_id
      connection_id -> ModelProviderConnection
      authentication = WorkloadIdentity
                     | StoredCredential
                     | DelegatedIdentity
                     | MutualTlsIdentity
                     | NoCredential
      configuration_digest
      lifecycle_state (includes active | draining | revoked)

    ModelAccessGrant
      grant_id
      session_id
      execution_attempt_id
      agent_revision_ref + agent_configuration_digest
      session_execution_plan_digest
      route_role = Primary | Auxiliary(role)
      model_selection_digest
      resolved_model_route_digest
      audience
      confirmation_key_thumbprint
      expires_at
      lifecycle_state

    ResolvedModelRoute
      route_contract_version
      model_version_ref + definition_digest
      model_selection_digest
      provider_model_identifier
      protocol
      provider_driver_version_ref + definition_digest
      connection_ref:
        connection_id
        configuration_digest
      binding_ref:
        binding_id
        configuration_digest
      provider_subject_fingerprint?

    AuxiliaryModelRoute
      role
      model_selection_digest
      route -> ResolvedModelRoute

ModelProviderConnection and CredentialBinding are reusable, hierarchy-scoped
control-plane resources. ModelProviderDriverVersion is an immutable platform
release of provider request and response translation behavior.
ResolvedModelRoute is immutable Session-owned data, not an independently
mutable resource. SessionExecutionPlan contains one primary ResolvedModelRoute
and zero or more typed AuxiliaryModelRoute values. Every primary and auxiliary
selection comes from the pinned AgentConfiguration. Each auxiliary value
records its typed role, selection digest, and route. The canonical
SessionExecutionPlan digest commits to the AgentRevision, AgentConfiguration
digest, exact selections, roles, driver versions, and routes.

ModelAccessGrant is attempt-scoped live authorization. Its identity and
confirmation key are not ResolvedModelRoute or SessionExecutionPlan content.
Every ExecutionAttempt receives new grants for the unchanged planned routes.

Each route records stable resource identities plus admitted definition and
configuration digests. It does not copy provider configuration, secret bytes,
an OpenBao secret version, a temporary cloud credential, a bearer token, or a
provider-native credential locator. The referenced control-plane resources
remain subject to live authorization, lifecycle, rotation, and revocation
checks. The driver release remains subject to live availability and revocation
checks.

A provider driver version pins every behavior that can change provider-visible
requests or model-visible responses, including protocol mapping, request
serialization, streaming assembly, tool-call translation, finish-reason
mapping, and response normalization. A mutable deployment version cannot
qualify. Changing the driver requires a new Session and never rewrites a
running plan.

The driver adapts an already-pinned ModelSelection to an external provider. It
is therefore a Session route fact rather than AgentConfiguration behavior.
Verification and outcome claims that depend on provider translation record the
exact ResolvedModelRoute and driver version they exercised.

Before forwarding a request, the model access service verifies that its loaded
driver artifact and translation contract match the version and definition
digest in ResolvedModelRoute. A mismatch fails closed with
`ProviderDriverMismatch`.

ModelProviderConnection identifies one provider security and billing
boundary. Provider, endpoint, region, account locator, allowed models, and
supported protocols use provider-specific value objects. They are not a
free-form map and are not encoded into a display name.

Each allowed-model entry binds one exact ModelVersion reference and definition
digest to the provider model identifier and any immutable provider revision or
deployment fingerprint the provider exposes. A mutable alias, `latest`,
`default`, or `auto` value cannot establish that binding. A platform catalog
record cannot make mutable upstream behavior immutable. When the exact
ModelProviderDriverVersion cannot prove that the configured provider offering
serves the pinned ModelVersion, route admission fails with
`ProviderModelMismatch`.

CredentialBinding identifies exactly one authorized authentication method for
one ModelProviderConnection:

- WorkloadIdentity names a deployment-attested identity or role-binding
  procedure that produces short-lived provider credentials. It contains no
  long-lived access key.
- StoredCredential contains a typed credential kind and a stable SecretRef.
  The current secret version remains behind SecretStore.
- DelegatedIdentity contains the external subject and a SecretRef for durable
  refresh or client material. Access tokens are minted and cached
  ephemerally by the model access service. DelegatedIdentity is not
  interchangeable with an API key.
- MutualTlsIdentity contains typed certificate and private-key or KeyRef
  references.
- NoCredential is valid only for a typed auth-free local endpoint created by
  deployment configuration. It is invalid for tenant-supplied or remote
  network endpoints.

Exactly one active CredentialBinding is eligible for new route admission on a
ModelProviderConnection. A replaced binding may be draining only for Sessions
that already pin it under section 7. The owning hierarchy node determines
whether a connection is platform, tenant, project, or self-hosted deployment
scope. Policy grants use of the connection. Ownership and authorization are
not encoded into the provider or model name.

Connection and binding configuration is immutable. Their configuration
digests exclude lifecycle state and the current secret or temporary credential
version. A provider, endpoint, provider subject, authentication kind,
SecretRef, protocol, allowlist, or parent change creates a new resource and
digest. Validated secret rotation behind one stable SecretRef changes neither.

### 2. Resolve every route before a Session becomes runnable

[ADR#0031](./0031-agent-implementation-and-session-plan.md) owns the overall
Session admission order and the single atomic `SessionStarted` write. Its model
route resolution step uses this subprocedure:

1. Derive tenant, hierarchy scope, and principal from authenticated context.
2. Load the exact primary and typed auxiliary ModelSelection values from the
   pinned AgentRevision and verify their AgentConfiguration and model
   definition digests. A Session or implementation cannot add, remove, or
   override a selection or deterministic parameter.
3. For every route, select an explicitly requested authorized
   ModelProviderConnection or the single applicable policy default at the
   Session hierarchy scope.
4. Load each connection's single active CredentialBinding.
5. Resolve one exact authorized ModelProviderDriverVersion for the provider and
   protocol. Missing, ambiguous, mutable, or unsupported driver selection
   fails admission.
6. Verify live authorization, lifecycle state, the exact provider offering
   for the selected ModelVersion, provider subject, auxiliary role, driver
   capabilities, and the implementation compatibility requirements supplied by
   [ADR#0031](./0031-agent-implementation-and-session-plan.md).
7. Build the primary and typed auxiliary route values, including the exact
   driver versions, and compute every route digest.
8. Return those immutable route values to the overall admission coordinator.
   It resolves the remaining dependencies, builds the native configuration
   projection, computes the complete SessionExecutionPlan digest, and records
   the plan atomically with the sole `SessionStarted` fact under that decision.

No grant is placed in the immutable plan. After SessionStarted is durable, the
platform may launch an ExecutionAttempt. Its supervisor creates a fresh
confirmation key, and Ready attests the key thumbprint together with the
attempt and plan. After validating Ready, the coordinator requests one
ModelAccessGrant per route from the model access service. The service creates
and activates each grant, bound to the ExecutionAttempt, AgentRevision,
AgentConfiguration digest, ModelSelection digest, route role, route digest,
SessionExecutionPlan digest, and confirmation-key thumbprint. The Session is
not runnable until every required activation succeeds. Activation failure
revokes all pending or partially activated grants and fails the attempt.
Reconciliation revokes orphaned pending grants after a bounded timeout.

Ending an attempt revokes its grants. A replacement attempt creates a new key
and new grants for the same immutable routes only after its Ready evidence is
validated. No proof private key or renewal authority must survive the failed
supervisor, and restart never changes the plan.

A missing, ambiguous, unauthorized, inactive, digest-mismatched, or
incompatible route for any revision-bound selection rejects Session start.
Resolution never probes a list of credentials, chooses the first credential
that works, or silently uses a platform-funded account.

The selected models, roles, parameters, protocols, provider drivers,
connections, bindings, and implementation compatibility result do not change
during the Session. A different model, role, parameter, or implementation
requires a Proposal, a new AgentRevision, and a new Session. A different
provider account, endpoint, protocol, driver, or binding requires a new
Session. No change edits a running plan.

### 3. Keep provider credentials out of agent, implementation, and execution state

An AgentConfiguration owns exact ModelSelection values and deterministic
parameters. It contains no ModelProviderConnection id, CredentialBinding id,
SecretRef, provider token, auth profile, environment variable name, or
provider-native credential locator.

Provider connection metadata and credential lifecycle state are security-plane
product state. Secret input is write-only and goes directly to the platform
secrets service. Read APIs return redacted metadata and stable binding
identity, never the submitted value.

Durable commands, events, snapshots, and projections may carry connection
ids, binding ids, credential kinds, subject fingerprints, lifecycle state, and
failure categories. They never carry raw values, authorization headers, OAuth
refresh tokens, AWS access keys, signed AWS requests, or provider-native
secret locators.

Generated implementation configuration, execution environment, agent exports,
diagnostics, resolved-configuration views, ACP messages, prompts, transcripts,
checkpoints, logs, traces, and crash artifacts contain no upstream credential.
An AgentImplementationAdapter never receives a generic secret bundle.

Credentials bind to the component and purpose that consume them:

- a CredentialBinding attached to exactly one ModelProviderConnection owns the
  model provider authentication method and any SecretRef;
- host and supervisor credentials belong to the launch and deployment security
  plane; an ExecutionAttempt records their authenticated subject or attestation
  evidence but does not own reusable credential configuration;
- channel bot tokens belong to the channel adapter; and
- tool credentials belong to tool execution.

Reusing the same secret material for more than one purpose still requires
separate typed bindings. A remote implementation that requires the platform to
copy a tenant provider key into a second vault is unsupported. It is an
external delegated agent outside the hosted model-access guarantees unless a
later ADR explicitly amends
[ADR#0023](./0023-secret-management-and-key-custody-direction.md)'s custody
boundary.

### 4. Broker hosted model access through a session-scoped proxy

The hosted platform adds a model access service. It is the only hosted
platform component that calls upstream model providers. The secrets service
remains the only OpenBao client.

    Native AgentImplementation
      -> Session-bound local or private model endpoint
      -> platform-controlled attested supervisor
      -> sender-constrained ModelAccessGrant token
      -> model access service
      -> SecretStore resolution or provider workload identity
      -> model provider

For each ExecutionAttempt, a platform-controlled supervisor outside the native
implementation creates and retains a fresh confirmation key. Ready presents
its thumbprint. After Ready validates, the model access service issues the
attempt-scoped grants and a bounded renewal capability to the supervisor. The
supervisor retains that capability but cannot issue or widen a grant. It
exposes only a Session-scoped provider-compatible model endpoint to the native
implementation. A local launch uses OS process identity, a private Unix socket,
mutual TLS, or an equivalent mechanism that prevents another Session from using
it.

A verified remote ExecutionAttempt requires the platform-controlled attested
supervisor defined by
[ADR#0031](./0031-agent-implementation-and-session-plan.md) beside the remote
implementation. The native product calls that supervisor over an authenticated
local or private channel. The supervisor binds every forwarded request to the
Session, plan digest, active execution attempt, route, operation id, and
request digest, then attaches the sender-constrained grant and proof. Its
attested identity and effective configuration must match the immutable
ExecutionAttempt and Ready evidence.

If the platform cannot establish that supervisor and attestation boundary, the
remote system is treated as an external delegated agent. This is a boundary
classification, not another platform resource type. The platform may authorize
delegation and record the result, but it does not provide a ModelAccessGrant or
provider credential and does not claim that the external system's model access
conforms to this ADR.

Each native request names the primary route or one declared auxiliary role.
The supervisor attaches that route's short-lived grant token and proof of
possession when forwarding the request. The native AgentImplementation never
receives the token, confirmation key, renewal authority, upstream credential,
or permission to resolve a SecretRef. The supervisor checks that the Session,
AgentRevision, AgentConfiguration digest, SessionExecutionPlan,
ExecutionAttempt, selected ModelSelection, ResolvedModelRoute, and
ModelAccessGrant remain authorized before renewal and cannot mint beyond
Session or ExecutionAttempt termination. A binding is eligible only when
active, or when explicitly draining for that already-pinned Session and its
replacement attempt grant.

The model access service authenticates and authorizes every request against
the live Session, AgentRevision, configuration digest, plan digest, execution
attempt, selected model and route role, route digest, connection, binding, and
grant. An auxiliary grant cannot authorize a primary call or another auxiliary
role. The service then performs exactly one of these operations:

- resolve a StoredCredential through the secrets service immediately before
  the upstream request;
- refresh a DelegatedIdentity without exposing durable refresh material to
  the implementation;
- acquire short-lived provider credentials from WorkloadIdentity;
- use the configured MutualTlsIdentity; or
- call a deployment-created auth-free local endpoint.

Resolved secret values exist only for the in-flight operation, subject to the
bounded caching and invalidation rules in
[ADR#0023](./0023-secret-management-and-key-custody-direction.md). The model
access service cannot select a different role, model, connection, binding, or
provider account from the selected ResolvedModelRoute.

A compromised implementation may spend within its live grant through its
bound supervisor. Proof of possession, short expiry, exact route
authorization, and Session-scoped access bound that risk. Budget and rate
enforcement belong at the model access service but their product policy is
outside this ADR.

### 5. Use workload identity for Bedrock by default

Amazon Bedrock is a model provider, not an AgentImplementation or hosting
selection. A Bedrock ModelProviderConnection records the validated AWS account
boundary, region, endpoint class, allowed models, and supported protocol.

Its default CredentialBinding is WorkloadIdentity. The model access service
uses a deployment-attested identity to assume a narrowly scoped role and
obtains short-lived AWS credentials. Long-lived AWS access keys are not
stored. Trust policy, session policy, external identity, account, and role
binding are typed configuration rather than arbitrary strings.

A Bedrock API key, when deliberately supported, is a StoredCredential and
follows the same SecretRef path as any other provider key. Workload identity
failure never falls back to an access key or another AWS account.

Selecting Bedrock does not select AWS AgentCore Harness or AgentCore Runtime.
Selecting an AWS execution product does not authorize it to call Bedrock. The
model access service remains the sole upstream caller. A verified remote AWS
implementation requires the platform-controlled attested supervisor. Without
it, the AWS product is treated as an external delegated agent and cannot
receive platform provider credentials or duplicate them in AgentCore Identity.

### 6. Project the route without native defaults or fallback

Each implementation adapter projects the exact primary and auxiliary
ModelSelection values from AgentConfiguration plus their ResolvedModelRoute
values into the smallest native configuration needed to call the Session-bound
model proxy. Native files and profiles are generated artifacts, not sources of
truth.

Every adapter must set the exact provider model identifier, protocol, and
proxy endpoint for each selection plus the exact revision-bound implementation
mode. It must inspect the effective native configuration before the Session
becomes runnable. Its digest must match SessionExecutionPlan and Ready
evidence. A mismatch is a typed configuration failure, not a reason to
continue with a native default.

The following behavior is prohibited:

- defaulting to a provider or model because the platform omitted one;
- selecting the first configured provider or model;
- rotating among native auth profiles or provider accounts;
- changing provider or model after admission;
- failing over to another provider, model, billing account, endpoint, or
  implementation;
- retrying an authentication failure with another credential binding; and
- using display strings or native error text to choose a route.

Adapter-specific requirements include:

- Codex receives an explicit model and custom provider route to the local
  proxy. Hosted execution does not seed shared auth state, copy a personal
  login, inject an upstream API key, or expose an AWS credential chain to
  Codex. A direct built-in provider path is non-conforming when it bypasses
  the platform model access service.
- Claude Code receives the exact revision-bound model rather than an inherited,
  default, or auto-selected model, plus only the generated provider route to
  the proxy. User, project, or shared settings cannot substitute another model,
  provider profile, login, or credential. Hosted execution does not expose an
  upstream API key or provider credential chain to Claude Code.
- When OpenClaw is a fully pinned standalone implementation or a behaviorally
  significant component of a composite implementation, it receives the exact
  provider/model for each Session-planned route, only the generated platform
  profiles required by those routes,
  exactly one eligible profile per route, empty model-fallback chains, and no
  alternate credential candidates. Provider/model-scoped `agentRuntime.id`
  must be derived from and match the implementation component pinned by
  AgentConfiguration. Unset, default, and auto implementation selection are
  prohibited because OpenClaw can otherwise select a component from the
  provider/model route or fall back to an embedded loop. Native credential
  rotation, provider failover, model failover, and implementation fallback are
  disabled. If effective configuration cannot prove those facts, that
  OpenClaw combination is unsupported.
- When OpenClaw only transparently hosts a Codex implementation, OpenClaw
  receives no independent model route or implementation selection. The
  platform projects each ModelSelection and ResolvedModelRoute directly to the
  Codex adapter through the attested supervisor. If the OpenClaw host requires
  its own provider, model, or loop choice, the arrangement is composite rather
  than transparent and follows the preceding rule.
- An unpinned or auto-selecting OpenClaw deployment is treated as an external
  delegated agent. Its internal model access is outside the hosted guarantees
  and receives no platform grant or provider credential.
- The managed default loop calls the model access service through its typed
  client and has no independent provider-selection path.
- A verified remote implementation receives only the exact projected route
  through its platform-controlled attested supervisor. It becomes an
  external delegated agent when its API can replace that route with a vendor
  default, switch providers during the Session, or requires provider
  credentials in a vendor token vault.

ACP may carry non-secret provider capability metadata. It does not select or
authenticate ResolvedModelRoute. Upstream credentials, provider authorization
headers, platform grant tokens, confirmation keys, and renewal authority are
prohibited in every ACP message.

A self-hosted development adapter may use
[ADR#0007](./0007-configuration-sources.md)'s explicit environment bootstrap
only when it is marked development-only, isolated from tenant workloads, and
unable to serialize the value into durable state. That exception does not
conform to the hosted model access path.

### 7. Validate registration, rotation, and revocation fail closed

Initial registration stages connection metadata and credential material,
performs a bounded provider probe, records the verified provider subject
fingerprint when the provider exposes one, and activates only after every
check succeeds. A probe must use a provider operation that does not create an
unbounded billable model invocation.

A secret-backed rotation follows this order:

1. Write a new secret version behind the existing SecretRef.
2. Validate it against the expected provider subject and billing boundary.
3. Activate the version atomically.
4. Publish cache invalidation to every secrets-service and model-access
   replica;
5. retire the old version after the bounded overlap; and
6. record the rotation outcome without recording secret material.

There is never more than one active CredentialBinding for the connection.
Rotation does not make resolution depend on whichever credential works first.
Validated rotation behind the same SecretRef and provider subject does not
change ResolvedModelRoute, mint an AgentRevision, or restart a Session.

A WorkloadIdentity trust or role change creates a new CredentialBinding under
the same connection only when the adapter proves continuity of provider
subject and billing boundary. An endpoint, account, delegated subject, custody
owner, or unproven provider identity change creates a new
ModelProviderConnection.

Replacing one CredentialBinding with another never rewrites a running
ResolvedModelRoute. Before activation, the replacement operation finds every
live Session whose plan pins the old binding and applies one explicit mode:

- `Drain` marks the old binding draining, activates the new binding for new
  Session admission, and permits only already-pinned Sessions to keep using the
  old binding. No new Session route may select the draining binding. A
  replacement ExecutionAttempt for an already-pinned Session may receive new
  attempt-scoped grants for that same route. When the last pinned Session
  terminates, the old binding is revoked.
- `FailIfInUse` rejects replacement while any live Session pins the old
  binding. The old binding remains active and no partial lifecycle transition
  occurs.

Policy must explicitly permit the bounded overlap used by `Drain`. If overlap
is unsafe or the old credential must stop immediately, the platform revokes
the old binding and running Sessions fail with `CredentialRevoked` or
`CredentialBindingSuperseded`. They never switch to the new binding. A
security revocation always takes the immediate fail-closed path and never
drains.

Revoking a connection, binding, grant, or its policy authorization takes
effect on the next model request. That request fails closed. Revocation never
causes the Session to use another provider, endpoint, model, credential, or
billing account. Grant renewal stops when its ExecutionAttempt terminates, the
Session terminates, or any referenced resource becomes inactive.

### 8. Treat provider endpoints as security-sensitive routing data

A customer-supplied model provider endpoint requires:

- authenticated HTTPS;
- hostname verification;
- explicit trust roots;
- disabled redirects;
- DNS-rebinding-safe resolution;
- connection to the validated address;
- egress policy that rejects loopback, link-local, metadata, and undeclared
  internal destinations; and
- revalidation before any credential-bearing request after DNS or
  configuration changes.

Provider-native endpoints are derived server-side from validated provider
resources and regions. Callers cannot supply arbitrary provider parameters,
authorization headers, host overrides, redirects, or trust settings.

When a provider exposes a safe stable account or subject identity,
ModelProviderConnection records its non-secret fingerprint as the continuity
anchor. Registration and rotation reject a different subject. If the adapter
cannot prove continuity, it creates a new connection instead of claiming a
rotation.

Endpoint, protocol, and model capability validation occurs before credential
resolution. A rejected endpoint must never receive a credential-bearing
request.

### 9. Use typed failures and route-preserving retry rules

The domain failure contract distinguishes at least:

| Failure | Meaning | Automatic retry |
| --- | --- | --- |
| ModelRouteUnavailable | Route is missing, ambiguous, inactive, or denied | No |
| AgentRevisionModelMismatch | Requested selection or parameters differ from the pinned AgentRevision | No |
| ModelDefinitionDigestMismatch | ModelVersion definition does not match the revision pin | No |
| ProviderModelMismatch | Provider offering does not serve the exact selected ModelVersion | No |
| ProviderDriverMismatch | Driver version, artifact, or translation contract differs from the planned route | No |
| ModelNotAllowed | Connection does not allow the resolved model | No |
| ModelProtocolUnsupported | Implementation or connection cannot use the protocol | No |
| ModelRouteConfigurationMismatch | Native effective configuration differs from the plan | No |
| ModelAccessGrantMismatch | Grant is bound to another attempt, revision, plan, selection, role, route, or key | No |
| ExecutionAttemptMismatch | Request sender or attestation differs from the active execution attempt | No |
| RemoteSupervisorUnattested | Verified remote access lacks the required attested supervisor | No |
| CredentialUnavailable | Binding cannot produce a credential | No |
| CredentialBindingSuperseded | A replacement invalidated the binding pinned by this Session | No |
| CredentialRevoked | Binding, grant, or policy was revoked | No |
| ProviderAuthenticationRejected | Provider rejected the admitted credential | No |
| ProviderIdentityMismatch | Provider subject differs from the connection | No |
| ProviderThrottled | Provider explicitly requested bounded backoff | Same route only |
| ProviderQuotaExhausted | Provider account has no usable quota | No |
| ProviderUnavailable | Provider transport or service failed before acceptance | Same route only when safe |
| ProviderOutcomeUnknown | Request may have been accepted or billed | No |
| InternalModelAccessFailure | Adapter or model access invariant failed | No |

Adapters map native outcomes into these variants without matching display
text. An unmapped native error is InternalModelAccessFailure, not permission
to select another route.

Each model request has a stable ModelInvocationId. An automatic retry is
allowed only through the same ResolvedModelRoute and only when:

- the adapter proves that the provider did not accept the request; or
- the provider offers a documented idempotency contract and the retry reuses
  the same provider idempotency key.

If transmission may have reached the provider and no idempotency guarantee
settles the outcome, the result is ProviderOutcomeUnknown. A streaming request
that emitted output is not automatically replayed unless the provider offers a
documented continuation or idempotency contract that prevents duplicate
execution and billing.

Authentication rejection, throttling, quota exhaustion, provider
unavailability, and unknown outcome never trigger another model, provider,
connection, credential, endpoint, implementation, or external delegated agent.
Every mismatch variant fails closed without substituting another revision,
model, provider route, binding, supervisor, or execution attempt.

### 10. Audit the route without logging credential material

Every model request records enough non-secret context to reconstruct who used
which provider account and under which execution plan:

- tenant, principal, Session, Agent, and AgentRevision;
- AgentConfiguration, AgentImplementationVersion, exact primary and auxiliary
  ModelSelection values, and all definition and configuration digests;
- SessionExecutionPlan, its plan digest, and the active ExecutionAttempt and
  Ready attestation references supplied by
  [ADR#0031](./0031-agent-implementation-and-session-plan.md);
- route role, ModelVersion, selection digest, provider model identifier,
  protocol, ModelProviderDriverVersion and definition digest,
  ModelProviderConnection, CredentialBinding, ModelAccessGrant, and every
  admitted route digest;
- provider subject fingerprint when safely available;
- ModelInvocationId, provider request id, timing, outcome category, and usage
  attribution; and
- one correlation id joining model-access, secrets-service, and provider audit
  records.

Logs, traces, metrics, errors, and audit records redact authorization headers,
secret values, refresh material, signed AWS requests, grant tokens,
confirmation keys, and renewal authority. Audit records identify the stable
SecretRef-backed binding, never the secret version's plaintext.

An adapter and provider combination is not supported until automated proof
shows:

- no upstream credential enters Agent, AgentConfiguration, native
  implementation, execution attempt, ACP, prompt, transcript, checkpoint,
  JetStream, NATS KV, NATS Object Store, database, backup, log, trace, or crash
  state;
- grant tokens, confirmation keys, and renewal authority exist only in the
  platform-controlled supervisor's bounded memory and never enter the native
  implementation or durable state;
- a tenant cannot select, infer, or spend through another tenant's connection;
- another Session cannot use the local or private proxy channel or copied
  grant token;
- a verified remote request cannot proceed without the attested supervisor and
  active ExecutionAttempt bound to its plan;
- an external delegated agent receives no platform grant or provider
  credential;
- provider-key rotation invalidates every cache within
  [ADR#0023](./0023-secret-management-and-key-custody-direction.md)'s bound;
- connection, binding, grant, and policy revocation fail the next request
  closed;
- binding replacement drains only already-pinned Sessions or fails without
  partially switching them;
- Bedrock workload-identity renewal failure does not fall back to stored
  credentials;
- customer endpoint controls reject redirects, DNS rebinding, untrusted
  certificates, hostname mismatch, metadata endpoints, loopback, link-local,
  and undeclared internal destinations;
- native provider, model, credential, and implementation fallback cannot
  bypass the revision-bound ModelSelection or plan-owned ResolvedModelRoute;
- provider requests and responses use the exact driver artifact and translation
  contract pinned by ResolvedModelRoute;
- ProviderOutcomeUnknown is not replayed automatically;
- audit correlation reconstructs a call without exposing credential material;
  and
- crash cleanup removes temporary provider credentials and in-flight secret
  copies.

## Consequences

- Agent definitions remain portable across platform-funded and tenant-funded
  model access without embedding security state in revisions.
- AgentConfiguration owns every exact model selection and AgentRevision binds
  that immutable configuration, while SessionExecutionPlan records the
  provider route and credential binding admitted to serve it. Rotation and
  revocation remain live security operations.
- Codex, Claude Code, fully pinned OpenClaw, the managed loop, Bedrock, and
  non-Bedrock combinations use one typed provider boundary instead of
  implementation-specific credential fields.
- The hosted platform gains a model access service on the model-call hot path.
  It must stream responses, enforce live policy, preserve idempotency
  semantics, and remain highly available.
- Native implementation compromise can spend only through its bound supervisor
  and within the live grant envelope, but can still consume budget during that
  window. Budget and rate enforcement remain required follow-up work.
- Native provider and model failover are unavailable unless a later ADR
  defines an explicit, ledgered Session transition with equivalent custody,
  billing, idempotency, and verification guarantees.
- A verified remote implementation requires a platform-controlled attested
  supervisor. An unpinned remote product or one that insists on its own
  provider-key vault is an external delegated agent outside these hosted
  guarantees.
- Channel credentials, tool credentials, host operational authentication,
  model catalogs, pricing, and provider driver internals beyond the exact
  version and translation-contract pin remain separate decisions.

## References

- [ADR#0007: Configuration Sources](./0007-configuration-sources.md)
- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR#0020: ACP SDK 1.x Boundary and Bridge-Owned Callback Traits](./0020-acp-sdk-1x-boundary-and-bridge-traits.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0025: Agent Definition Data Ownership](./0025-agent-definition-data-ownership.md)
- [ADR#0031: Agent Implementation and Session Plan](./0031-agent-implementation-and-session-plan.md)
- [ACP conformance matrix](../architecture/acp-conformance.md)
- [Agent platform decision record](../research/agent-platform/decision-record.md)
- [Claude Code Agent SDK research dossier](../research/agent-platform/products/claude-code-agent-sdk.md)
- [OpenClaw research dossier](../research/agent-platform/products/openclaw.md)
- [AWS AgentCore Harness research dossier](../research/agent-platform/products/aws-agentcore-harness.md)
- [Codex App Server](https://developers.openai.com/codex/app-server)
- [Codex custom model providers](https://learn.chatgpt.com/docs/config-file/config-advanced#custom-model-providers)
- [Codex authentication](https://learn.chatgpt.com/docs/auth)
- [Codex with Amazon Bedrock](https://learn.chatgpt.com/docs/amazon-bedrock)
- [Claude Agent SDK](https://platform.claude.com/docs/en/agent-sdk/overview)
- [OpenClaw agent runtimes](https://docs.openclaw.ai/concepts/agent-runtimes)
- [OpenClaw model failover](https://docs.openclaw.ai/model-failover)
- [OpenClaw secrets management](https://docs.openclaw.ai/gateway/secrets)
- [OpenClaw model authentication](https://docs.openclaw.ai/gateway/authentication)
- [Amazon Bedrock AgentCore Harness model configuration](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/harness-models.html)
- [Amazon Bedrock AgentCore credential management](https://docs.aws.amazon.com/bedrock-agentcore/latest/devguide/security-credentials-management.html)
