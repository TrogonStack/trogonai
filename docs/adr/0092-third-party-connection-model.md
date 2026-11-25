---
number: "0092"
slug: third-party-connection-model
status: draft
date: 2026-11-02
---

# ADR#0092: A Third-Party Connection Is a Consented Provider Authority Pinned to an Immutable Connector Release and Provider App, and Its Aggregates Hold Only Secret References

## Context

[ADR#0063](./0063-agent-connection-declarations.md) lets an Agent declare the
connections it needs as a `ConnectionSelector` whose `provider` is an open
identifier, and leaves the connection resource for non-model providers to the
security plane. [ADR#0032](./0032-model-route-and-credential-binding.md) gave
model providers that resource: a `ModelProviderConnection`, a binding that
holds a stable `SecretRef`, and an attempt-scoped grant. Nothing equivalent
exists for Linear, Slack, Notion, GitHub or a remote MCP server, so
there is no place to say which provider account a credential belongs to, which
OAuth client obtained it, which reviewed description of the provider it was
obtained under, or what must happen when any of those change.

The custody rules already exist.
[ADR#0023](./0023-secret-management-and-key-custody-direction.md) makes the
platform secrets service the only OpenBao client and gives every consumer an
opaque `SecretRef`, [ADR#0074](./0074-credential-platform-extraction-boundary.md)
fixes what moves into that service, [ADR#0079](./0079-external-secret-sources.md)
keeps external sources read-only, and
[ADR#0083](./0083-sealed-content-envelope.md) seals credential records and the
streams of customer managed tenants.

The code on main has none of the connection side. The gateway's only Linear
integration is inbound: `LinearConfig` carries a webhook secret and nothing
else (`rsworkspace/crates/platform/trogon-gateway/src/source/linear/config.rs:26`),
and the runtime route resolves `CredentialKind::WebhookSecret` through the
runtime projection to verify `linear-signature`
(`rsworkspace/crates/platform/trogon-gateway/src/source/linear/server.rs:246`).
No outbound OAuth exists. The store behind it writes one string per credential
as `{"data":{"value": ...}}` with no `options.cas`
(`rsworkspace/crates/platform/trogon-gateway/src/secret_store/openbao_secret_store.rs:479`),
`CredentialKind` has no OAuth token set
(`rsworkspace/crates/platform/trogon-gateway/src/credential/commands/domain/credential_kind.rs:4`),
and all of it runs in the internet-exposed gateway that
[ADR#0074](./0074-credential-platform-extraction-boundary.md) says to extract.

This ADR decides what a connection is, what it pins, when it is the same
connection and when it is a new one, and what may never be written into its
state. The consent protocol is [ADR#0094](./0094-oauth-authorization-protocol.md),
token custody and refresh are
[ADR#0095](./0095-connection-credential-custody-and-refresh.md), ownership and
delegation are [ADR#0093](./0093-connection-ownership-and-delegation.md), and
use by Agents is [ADR#0096](./0096-connection-use-by-agents.md).

## Decision

**A connection is one consented authority at one provider account, held under an
explicit owner, authenticated through a pinned connector release and a pinned
provider app. Platform state refers to its material only by opaque
`SecretRef`.**

Agent-originated provider requests and responses use the HTTP proxy boundary of
[ADR#0096](./0096-connection-use-by-agents.md). The proxy authorizes the request,
obtains the credential from custody, injects it and forwards HTTP. These
protobuf aggregates describe lifecycle and authorization; they do not turn every
provider API endpoint into an RPC. The [connection architecture
explanation](../architecture/third-party-connections.md) separates these concerns.
The former operation-catalog fields are retired and reserved. `ProxyProfile`
now declares credential placement, request restrictions, resource limits and
quota feedback. `IdentityProbe` directly declares a fixed reviewed HTTP GET or
GraphQL query instead of naming an ordinary provider operation.

### 1. Separate resources, each with its own stream, and none holds material

| Resource | What it is | Scope of its stream |
| --- | --- | --- |
| ConnectorRelease | Immutable, digest-pinned description of how to talk to one provider | Platform |
| ProviderApp | The OAuth client, GitHub App or dynamically registered client the platform presents to the provider | Platform, or a customer project |
| AuthorizationAttempt | One consent round trip ([ADR#0094](./0094-oauth-authorization-protocol.md)) | Custody project |
| ProviderAccountClaim | The exclusive right to bind one provider account | Custody project |
| Connection | One consented authority plus its credential bindings | Custody project |
| ConnectionGrant | Attempt-scoped permission to use one connection ([ADR#0096](./0096-connection-use-by-agents.md)) | The Session's project, not the custody project |

Each is its own aggregate under [ADR#0045](./0045-event-sourced-service-module-layout.md)
because the lifetimes differ: a release outlives every connection pinned to it, a
platform app is shared by many tenants, an attempt lasts minutes, a grant lasts
one execution attempt. Project-scoped subjects are
`{tenant}.connections.<aggregate>.events.v1alpha1.{project}.{id}` and carry
opaque ids only, never an account id, email or URL.

That project template does not apply to the platform catalog or platform apps.
Their streams belong to the platform operator namespace and retain their typed
parent; a transport adapter must not fabricate a ProjectId or route them through
the calling tenant's project. Stream identity includes resource kind, complete
parent and aggregate id. Concrete subject encoding and account bindings remain
part of transport integration under [ADR#0083](./0083-sealed-content-envelope.md).

No event, snapshot or idempotency record of any of them contains material. The
only secret-bearing field type in these packages is
`trogonai.secrets.v1alpha1.SecretRef`. Everything else is a fact about the
secret (generation, expiry, outcome), never the secret.

Resource containment is explicit in every routed command, event, present state,
reference and resource-name value as typed `resource_parent`. A project container
is `ProjectResourceParent`; it is not a hierarchy NodeId. The platform catalog,
platform apps and platform app material have explicit platform parent types.
A grant's parent is the Session project and may differ from Connection custody.
Logical `ConnectionOwner` and credential subject associations are separate.

Public create/list envelopes use a typed collection `parent`; get envelopes
use an identity-only resource-name message. Schema equality ties route payload,
command and returned state to the same parent, and authenticated dispatch checks
external ownership and namespace authority. The [resource-parent reference](../reference/connector-resource-parents.md)
defines all containers, names, equality rules and canonical formatting. Bare
hierarchy `parent` keeps its NodeId meaning under [ADR#0040](0040-contract-field-vocabulary.md) and [ADR#0066](0066-project-resource-and-members-only-access.md).

### 2. A ConnectorRelease is immutable bytes with a digest, and Blocked is terminal

A connection pins `ConnectorReleaseRef { resource_parent: ConnectorReleaseParent, release_id,
definition_digest }`. The `connector_id` is the value an Agent matches with
`ConnectionSelector.provider` ([ADR#0063](./0063-agent-connection-declarations.md)
Decision 3), so the catalog stays the security plane's and the Agent contract
never mirrors it. The digest covers the exact stored bytes of the definition,
which are never re-encoded.

| State | Meaning |
| --- | --- |
| Published | Selectable for new connections and usable by existing ones |
| Deprecated | Denies new selection only; existing connections keep working |
| Blocked | Terminal; denies live use immediately |

The commands are `PublishConnectorRelease`, `DeprecateConnectorRelease` and
`BlockConnectorRelease`; `Published -> Deprecated` and `Published | Deprecated
-> Blocked` are the only transitions. There is no in-place patch: a correction
is a new release. Blocking never auto-upgrades a connection.

A `ConnectorDefinition` declares:

- the authentication methods the provider supports;
- an `OAuthProfile`: authorize, token and revoke URLs, PKCE `S256` required,
  client authentication method, scope separator and normalization, refresh
  behavior, omitted-expiry policy, fixed public parameters by OAuth stage and
  strict response-field selectors/encodings. These are reviewed authentication
  metadata, distinct from forwarding ordinary provider API responses;
- the credential roles a connection must bind;
- the credential injection and destination restrictions the HTTP proxy must
  enforce, with the relay binding and proof profile defined in [ADR#0096](0096-connection-use-by-agents.md);
- optional provider-specific helpers, such as an identity probe or an MCP tool
  policy, without requiring an operation definition, input schema or output
  mapping for each ordinary provider API endpoint;
- the exact egress hosts, each an `EgressHost { host, port }` and never a
  pattern, matching [ADR#0072](./0072-agents-improve-through-recorded-candidates.md)
  Decision 3;
- the identity probe: which provider call yields the `ProviderAccount` and which
  fields of its answer may be kept (Decision 9);
- revoke and uninstall semantics, including whether the provider can revoke at
  all, so an unsupported revoke is recorded and never faked;
- webhook capability, when the provider has one.

### 3. A ProviderApp is the client identity, and rotating its secret keeps that identity

| Origin | Meaning |
| --- | --- |
| `PLATFORM_MANAGED` | Registered by the platform; shared by tenants |
| `CUSTOMER_SUPPLIED` | Registered by a customer; owned by a project |
| `DYNAMICALLY_REGISTERED` | Created at connect time by RFC 7591 registration |

The kind is one of `OAuthClientApp { client_id, client_secret SecretRef,
redirect_uris, scope_ceiling, issuer, discovery_snapshot_digest }`,
`KeyedApp { app_id, private_key SecretRef }` for GitHub Apps, which are not OAuth
clients, or `DynamicClient { issuer, discovery_snapshot_digest,
registration_access_token SecretRef }`. The Client ID Metadata Document form of
`OAuthClientApp`, whose `client_id` is the document URL and which has no secret,
carries the same `issuer` and snapshot digest. Registration is how an app came to
exist, not an authentication method of a connection.

States are `Pending -> Active`, `Active <-> Disabled`, `Active <-> Degraded`,
and any state to `Revoked` (terminal). `Degraded` is a recorded health fact set
from broker evidence that the provider rejected the client's authentication; it
clears on a successful provider call or a secret rotation. `Disabled` is the
owner's reversible pause.

Identity is the resource parent, issuer, registration method and `client_id`. A change of
`client_id` or issuer is a new app. A rotation of the client secret or private
key keeps the app's identity. Resolution
takes the custody project first, then the platform, and an
ambiguous match fails rather than choosing. Cleaning up a connection never
deletes the app, including one the platform shares.

### 4. A Connection separates custody from the logical owner, and identity is what the provider consented to

The stream lives in a custody project (`ProjectId`, the kinship of
[ADR#0066](./0066-project-resource-and-members-only-access.md) and
[ADR#0046](./0046-project-anchored-resource-hierarchy.md): a project is never a
node). The logical owner is a `ConnectionOwner` oneof of project, workspace or
person. Every arm exists in the schema from the first version; policy
rejects the person arm until it is enabled. What an owner means for sharing,
delegation and offboarding is [ADR#0093](./0093-connection-ownership-and-delegation.md).

```proto
message ConnectionRef {
  reserved 1;
  reserved project_id;
  ConnectionId connection_id = 2;
  Digest configuration_digest = 3;   // exact stored bytes, not re-encoded
  ConnectorReleaseRef connector_release_ref = 4;
  ProjectResourceParent resource_parent = 5;  // custody, not logical owner
}
```

| State | Entered by |
| --- | --- |
| Pending | `ReserveConnection`, before consent completes |
| Active | `ActivateConnection`, which commits configuration, bindings and claim receipt |
| Degraded | A health fact from custody, such as repeated transient failure |
| ReconnectRequired | Provider uninstall, `invalid_grant`, lost rotation, or a provider `401` that survives one forced refresh; reserved for dead credentials |
| Suspended | `SuspendConnection` with reason `OWNER_DISABLED` or `OPERATOR_PAUSE`; `ResumeConnection` returns it to `Active` |
| Revoked | `RevokeConnection`; terminal |
| Aborted | `AbortConnection` from Pending |

`Active` moves to `Degraded`, `ReconnectRequired` or `Suspended` and back to
`Active`. Any live state moves to `Revoked`. Activation events carry the exact configuration
bytes and digest, the `ProviderAccount`, the granted authority, the claim receipt,
the binding set and the `authorization_generation`. Refresh never appends to
this stream (see [ADR#0095](./0095-connection-credential-custody-and-refresh.md));
the exception is a health fact, which is appended. Custody and the connections
service are its only writers; the connection proxy and trusted MCP executor report provider-auth
observations to the connections service through `ReportConnectionHealth`
([ADR#0096](./0096-connection-use-by-agents.md)).
A `403` with `insufficient_scope` keeps the connection `Active` with health reason
`SCOPE_STEP_UP` and the missing scopes recorded; requests refused for those scopes fail
([ADR#0094](./0094-oauth-authorization-protocol.md)).
`SCOPES_REDUCED` is such a fact, a health reason on an `Active` connection and
not a trigger for `Degraded`. It applies at exchange and at refresh, where custody
reports the reduced granted authority; admission and the proxy read the
granted authority from the Connection projection.

Granted authority is recorded as `GrantedAuthority { scopes, resource_refs,
evidence }` with evidence `PROVIDER_REPORTED`, `PROBED` or `DOCUMENTED_OMISSION`.
Requested scopes never prove what was granted. Effects with an uncertain remote
result (code exchange, refresh, provider revoke) are recorded as an
`EffectOutcome` of `CONFIRMED`, `UNSUPPORTED`, `REJECTED`, `UNAVAILABLE` or
`UNKNOWN`, which keeps saga states out of the aggregate.

| Change | Result |
| --- | --- |
| Provider account, issuer, subject kind, OAuth client or owner differs | New connection |
| MCP canonical resource URI differs | New connection |
| Reconnect to the same account | Same `ConnectionId`; the account must match |
| Access or refresh token rotates behind a binding | Nothing visible: same `ConnectionRef`, same AgentRevision |
| Client secret or private key rotates | Same app, same connections |
| API key replaced under the same role | `ReplaceCredentialBindings`; generation increases, `ConnectionRef` is unchanged |
| Release upgraded | `UpgradeConnector`; same id, new `ConnectionRef`; added egress hosts must be acknowledged |

Revoking is the owner's intent and is terminal, so reconnecting means a new
connection. A provider-side uninstall is a fact about the world and moves the
connection to `ReconnectRequired` on the same id. An owner's or operator's disable
of one connection is the reversible `Suspended` state with its reason. It is a
state, not a policy hold, because
[ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
holds bind only to a policy subject (an Agent or a node). Admission and the
proxy refuses a suspended connection, and refresh pauses. Ending a delegation
is revoking it, not holding it
([ADR#0093](./0093-connection-ownership-and-delegation.md)).

A connection is usable only when its own state is `Active`, its pinned release is
not `Blocked`, and its app is not `Revoked` or `Disabled`. Release and app
standing are joined at admission and at each call, never copied onto connections
(Decision 10).

### 5. A CredentialBinding names a role and a SecretRef, and is not the model-plane binding

The Connection owns a value `CredentialBinding { role, secret_ref,
binding_generation }`, one per credential role the release declares. It shares
properties with the [ADR#0032](./0032-model-route-and-credential-binding.md)
binding recorded in the [glossary](../glossary/credentialbinding): it holds a
stable `SecretRef` rather than material, and a rotation keeps the binding's
identity. It is otherwise a different type in a different package: it is a child
of a Connection, it is keyed by a role from a pinned release, and it authorizes
nothing by itself. The [glossary](../glossary/credentialbinding.md) distinguishes the package-qualified
meanings.

An OAuth connection holds one role whose single secret is the whole token set.
`binding_generation` increases only when the secret behind a role is replaced
by a different one; versions behind the same `SecretRef` do not change it.

### 6. A ProviderAccountClaim makes dedupe exact

Before activation, a connection must hold the claim for its provider account. The
claim identity is a keyed fingerprint, computed in custody with a key no other
service holds, over the tenant, custody project, owner, provider, issuer, app
boundary, subject kind, account, subject and installation. An unkeyed digest is
rejected because provider account ids are guessable and the digest would confirm
them. The claim is created with a NoStream write precondition, so two concurrent
connects for the same account cannot both win. The dedicated custody
[fingerprint operation and rotation registry](../reference/connector-account-identity.md)
authorizes sealed probe input against an open attempt and preserves a canonical
claim across generation aliases. A canonical fingerprint may retain a retired
computation generation; rotation never silently renames a held/bound claim.

States are `Held(connection) -> Bound -> Released`. A claim is never stolen on a
timeout: the holding connection must first be aborted or revoked, which is what
releases it. Connecting the same account under a different owner or a different
app boundary is allowed; connecting it twice under the same one is not. The claim
holds the fingerprint and the holder, never the account id itself.

### 7. A generic `mcp` release takes its egress hosts from a pinned discovery snapshot

For a remote MCP server the platform is the OAuth client and the server is the
resource server, under the pinned MCP authorization specification (2026-07-28).
A generic `mcp` release pins that version and permits a per-connection server
URL. A curated server (Linear MCP, Notion MCP) may ship its own release with
fixed hosts and is treated like any other.

Registration is tried in the specification's order, and every outcome ends as a
ProviderApp:

1. a preregistered client, as a `CUSTOMER_SUPPLIED` `OAuthClientApp`;
2. a Client ID Metadata Document, when the authorization server advertises
   support: one platform-hosted document, so no per-tenant registration state;
3. RFC 7591 dynamic registration, as a `DYNAMICALLY_REGISTERED` `DynamicClient`;
4. otherwise the user enters client details, which is the preregistered case.

For a dynamically registered client the app identity includes the registration
method, and the registration secret and access token are secrets bound to the
app, not to the connection. The authorization server metadata, its bytes and
digest, is the discovery snapshot recorded on the ProviderApp. The protected
resource metadata is recorded on the Connection, because it is per resource.
Neither is secret. A connection stores the canonical resource URI and issuer it
was granted for. How discovery is
fetched safely and how the flow runs is [ADR#0094](./0094-oauth-authorization-protocol.md).

**Deviation from the egress rule, acknowledged by the connecting owner.** The rule is
[ADR#0072](./0072-agents-improve-through-recorded-candidates.md) Decision 3:
exact `EgressHost { host, port }` identities, absence is deny, built on the
exact-identity rule of
[ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md).
For a catalog release the hosts are written and reviewed in the definition. For a
generic MCP release they cannot be, so the hosts that may be called are the
endpoint origins in the pinned authorization server metadata and the pinned
protected resource metadata. The owner acknowledges the exact list at connect
time, as for `UpgradeConnector`, and the acknowledgement is recorded on the
Connection event. A change in the set of endpoint origins needs a new
acknowledgement; a digest change with the same origins is recorded without one.
Hosts outside the snapshot are denied, and the mediator, not the Agent, makes the
call, so no Session's
egress permission widens. A connection-derived egress arm remains an additive
change to [ADR#0072](./0072-agents-improve-through-recorded-candidates.md), not
made here.

### 8. Material never enters state, and each stream has one sealing class

Never in any event, snapshot, idempotency record, KV value, JetStream message,
log, trace or error: access, refresh and ID tokens, client secrets, private keys,
JWT assertions, registration access tokens, API keys, passwords, the PKCE
verifier, an authorization code, signed state, webhook signing secrets,
token-bearing URLs, provider token or error bodies, and `Authorization` headers.

Each stream has one sealing class, under
[ADR#0083](./0083-sealed-content-envelope.md) Decision 1:

- Token sets, staged bundles, transaction capsules and registration secrets are
  credential material, sealed for every tenant
  ([ADR#0095](./0095-connection-credential-custody-and-refresh.md) Decision 4).
- The Connection, authorization attempt, account claim, grant and connection audit
  streams, and the stream of a customer-supplied ProviderApp, are sensitive
  streams: sealed for customer managed tenants, and under the server cipher for
  others. [ADR#0083](0083-sealed-content-envelope.md) declares the session streams today, so naming these is an
  amendment (Consequences).
- The catalog and platform-managed apps stay unsealed. The class of a webhook
  endpoint stream is set when the webhook phase lands.

[ADR#0094](./0094-oauth-authorization-protocol.md) and
[ADR#0096](./0096-connection-use-by-agents.md) refer to this decision instead of
restating a class. Before any sensitive stream ships, the
[ADR#0057](./0057-decider-command-nats-binding.md) reply must return events as
stored or by id, which is already the cutover precondition in [ADR#0083](0083-sealed-content-envelope.md).
OpenBao KV v2 values stay outside a customer's key veto
([ADR#0033](./0033-key-custody-product-model.md) Decision 7), so what that means
for a customer managed tenant's token sets is
[ADR#0095](./0095-connection-credential-custody-and-refresh.md).

A contract test fails the build on any field in the connection packages whose
name matches `token|secret|password|verifier|code|key` unless it is typed
`SecretRef` or on a reviewed allow-list.

### 9. Connection metadata keeps the least personal data that still tells a human which account this is

A `ProviderAccount` in the Connection stream holds the subject kind, the opaque
ids of a non-person subject (organization, workspace, installation), and the
keyed account fingerprint of Decision 6. The release's identity probe lists the
response fields it may read, so what is captured is reviewed with the catalog and
not discovered later. Email addresses, avatar and profile URLs, phone numbers and
free-text profile fields are not captured. One bounded display name is captured
at activation and again only at reconnect, never on a timer.

The display name and the provider ids of a `USER` subject are personal data, and
no event carries them, because an event cannot be erased for one person. They
live in the erasable provider account profile of
[ADR#0093](./0093-connection-ownership-and-delegation.md) Decision 11, which the
stream references by the fingerprint. Personal data never appears in a subject, a
header, a metric label, a log, a trace, a claim or a denial record. The owner's
own label is a separate, editable field set by `UpdateMetadata`. Erasure and
retention after offboarding or tenant deletion are
[ADR#0093](./0093-connection-ownership-and-delegation.md).

### 10. Provider API deprecation and app compromise act on release and app standing, never on N connections

Release standing, app standing and connection state are read live, at admission
and at each call, by admission, the connection proxy and the trusted MCP executor.
They read a projection whose staleness is measured from a heartbeat, and they
refuse past a configured bound of at most 330 seconds, the
[ADR#0049](./0049-revocation-latency-target.md) cache TTL of 300 seconds plus up
to 30 seconds of jitter, and never fall back to the last projection they held.
This matches the bounded-live rule proposed for
[ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
(PR #751, in review). The [ADR#0049](0049-revocation-latency-target.md) target of 5 seconds at p99 still governs the
normal case. A Blocked release or Revoked app therefore costs one event, not a
write to every connection stream.

| Situation | Who acts | Command | What each connection sees |
| --- | --- | --- | --- |
| Provider announces an API sunset | Platform catalog maintainers | Publish the successor; `DeprecateConnectorRelease` with reason, optional successor and sunset time | `DEPRECATED` standing and the notice; still usable; absent from new selection |
| The sunset arrives, or a defect makes the release unsafe | Platform operator | `BlockConnectorRelease` with a reason code | Admission and invocation fail with a release-blocked reason; the connection state is unchanged |
| An owner moves to the successor | The connection's owner | `UpgradeConnector`, acknowledging added hosts and scopes | Same id, new `ConnectionRef`; scopes beyond the grant make it `ReconnectRequired` |
| Client secret or private key suspected leaked | Platform operator (platform app), or the project admin (customer app) | Rotate at the provider, then write the new secret through the secrets service | Nothing, unless the provider drops tokens on rotation: refresh then fails, the app is `Degraded`, and connections wait without being revoked |
| The client identity itself must be abandoned | Same | `Revoked` on the app, a new app, a new connection per owner | Old connections are unusable; owners reconnect as a new connection, and the old one is revoked once the new one is `Active` so admission does not see two |

The platform never upgrades a connection on an owner's behalf, because added egress
hosts and widened scopes need a person's acknowledgement. A platform operator
cannot edit a customer-supplied app; in an emergency it suspends the connection
with reason `OPERATOR_PAUSE`.
Provider-side revocation of a connection's tokens needs the app's client
credentials; when the app is gone at the provider, the cleanup records that
outcome as it is.

### 11. The Linear gateway source moves onto a Connection by dual-run, and nothing is re-keyed

What exists: an inbound-only integration. The runtime route is mounted at
`{path_prefix}/{integration.id}/webhook`
(`rsworkspace/crates/platform/trogon-gateway/src/source_plugin.rs:571`,
`rsworkspace/crates/platform/trogon-gateway/src/source/linear/server.rs:135`),
resolves `WebhookSecret` through `RuntimeCredentialResolver`, verifies the
signature, and publishes with `Nats-Msg-Id` set to the provider's webhook id
(`rsworkspace/crates/platform/trogon-gateway/src/source/linear/server.rs:393`).
The credential is in the `CREDENTIAL_EVENTS` stream
(`rsworkspace/crates/platform/trogon-gateway/src/credential/constants.rs:3`) with
its value in OpenBao at a path computed from its key
(`rsworkspace/crates/platform/trogon-gateway/src/secret_store/openbao_secret_store.rs:563`).

- **No connection material goes through the gateway's store.** The store has no
  compare-and-swap, one string per credential and no OAuth kind. Token sets are
  written by the secrets service after [ADR#0074](./0074-credential-platform-extraction-boundary.md)
  extraction, as [ADR#0095](./0095-connection-credential-custody-and-refresh.md)
  defines.
- **Outbound Linear is new, not a cutover.** The first Linear connection obtains
  outbound authority under a platform app. It touches nothing the gateway does
  today and is undone by revoking it.
- **Inbound is re-homed in the webhook phase, with the old path left intact.** A
  webhook endpoint bound to the Connection takes over verification in the later
  webhook ingress decision. Until a soak shows no disagreement, both paths verify
  the same deliveries: the connection-backed path runs in shadow, recording
  agreement with the legacy verdict and publishing to a subject nobody consumes.
- **The flip is per integration, and rollback is flipping back.** The legacy route
  stays mounted and its credential and projection untouched. The flip changes
  which path publishes, so rollback restores the old behaviour with no data
  movement. The legacy path sets `Nats-Msg-Id` from the provider webhook id; the
  connection-backed path must derive the same id, so a delivery published by both
  around the flip collapses in the duplicate window. Identical ids, and zero
  disagreement across a soak longer than the provider's retry window, gate the
  flip.
- **Retire afterwards.** The legacy `WebhookSecret` credential is destroyed with
  the existing saga and the static `LinearConfig` secret is removed.
- **Existing OpenBao entries are not rewritten.** They keep their recorded ids
  and the paths those ids determine under [ADR#0074](./0074-credential-platform-extraction-boundary.md)
  Q4 as amended, which is the only re-key. A Connection may adopt one by `SecretRef` as a
  borrowed binding, and connection cleanup never destroys a borrowed secret; the
  legacy saga does.

### 12. The packages keep one module per stream

As [ADR#0045](./0045-event-sourced-service-module-layout.md) lays out, all are
edition 2024 and `v1alpha1`, with `state/`, `checkpoints/`, `idempotency/` and
`projections/` only where the concern exists:

```text
proto/trogonai/connections/
  types/ catalog/ provider_apps/ authorization_attempts/ account_claims/
  connections/ grants/ admission/ access/ audit/ providers/<provider>/
proto/trogonai/secrets/        credentials/ exchange/ sources/   (additive)
proto/trogonai/webhooks/       later phase
proto/trogonai/policy/connection_permissions/
```

## Alternatives Considered

**A `parent: NodeId` placement for connections.** Rejected.
[ADR#0066](./0066-project-resource-and-members-only-access.md) states a project
is never a node, and project work references the project by kinship. The custody
`ProjectId` is that reference; the logical owner is separate.

**Owner arms named `shared` and `user`.** Rejected. Workspace sharing needs its
own arm, resolved through a designated integration project, and `person` is the
accurate name for the `user` arm.

**Disable and Enable as a policy hold on the connection.** Rejected. Holds bind
only to a policy subject, an Agent or a node, so a connection cannot carry one.
A reversible `Suspended` state with a reason does the same work where every
consumer already reads connection state.

**`oauth_apps`, with dynamic registration as a connection authentication kind.**
Rejected. GitHub Apps are not OAuth clients, and dynamic registration is how an
app is created, not how a connection authenticates.

**Active, Deprecated and Withdrawn release states.** Rejected. It cannot say
that a release is denied immediately, and Deprecated only denies new selection.

**Dedupe by projection alone.** Rejected. A projection lags the stream, so two
concurrent connects can both pass its check. A claim with a NoStream
precondition is exact and cheap, since connects are rare.

**Copying release or app standing onto each connection.** Rejected. Blocking a
widely used release would write to every pinned connection's stream and race the
very use it is meant to stop. A live join is one event and immediate.

**Generalising `ModelProviderConnection` to non-model providers.** Rejected. Its
binding authorizes one model provider connection with one authentication method;
connectors add roles, owners, accounts, claims and scopes, and coupling the two
would put OAuth lifecycle into the model plane.

**A separate provisioning package of saga states.** Rejected.
[ADR#0045](./0045-event-sourced-service-module-layout.md) keeps sagas out of
aggregates. `EffectOutcome` carries the one real need, an outcome that is
unknown.

**An unkeyed digest as the account claim key.** Rejected. Account ids are
guessable and an unkeyed digest confirms a guess.

**Listing a generic MCP server's hosts in the release.** Rejected. The hosts are
unknown until discovery runs against a URL the owner chooses.

## Non-Goals

- The consent protocol, callback handling and discovery safety
  ([ADR#0094](./0094-oauth-authorization-protocol.md)).
- Token storage, refresh and restore
  ([ADR#0095](./0095-connection-credential-custody-and-refresh.md)).
- Ownership, delegation, offboarding and erasure
  ([ADR#0093](./0093-connection-ownership-and-delegation.md)).
- Admission, grants, HTTP proxy forwarding and budgets
  ([ADR#0096](./0096-connection-use-by-agents.md)).
- Webhook ingress beyond the Linear cutover constraints in Decision 11.
- Individual provider catalogs, and Trogon acting as an MCP resource server.
- Changing model-plane connections under [ADR#0032](./0032-model-route-and-credential-binding.md).

## Consequences

- New contract packages, additive. Connections, claims and attempts are new
  streams; no existing stream changes shape.
- Amendments to name and make:
  - [ADR#0083](./0083-sealed-content-envelope.md): classify the streams of
    Decision 8 as sensitive.
  - [ADR#0063](./0063-agent-connection-declarations.md): `provider` equals
    `ConnectorId`, and a ConnectionSelector resolves to a Connection.
  - [ADR#0074](./0074-credential-platform-extraction-boundary.md): connections
    are a second consumer, and an OpenBao policy for runtime token-set writes is
    needed, since none of the five policies covers it.
  - [ADR#0046](./0046-project-anchored-resource-hierarchy.md): custody against
    owner, and the integration project, which
    [ADR#0093](./0093-connection-ownership-and-delegation.md) carries.
  - [ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
    and [ADR#0072](./0072-agents-improve-through-recorded-candidates.md): the
    connection permission binding, and the snapshot-derived host of Decision 7.
- Glossary entries are needed for ConnectorRelease, ProviderApp, Connection,
  ProviderAccountClaim, the connector-sense credential binding and the discovery
  snapshot, and the existing `CredentialBinding` entry must point to the new one.
- The connection proxy and the trusted MCP executor default to Enforce and refuse
  to start without a policy source, per [policy enforcement posture proposal](https://github.com/TrogonStack/trogonai/pull/745) (policy enforcement posture, in
  review) Decision 1 and Decision 8; the decider hosts for connection, attempt,
  claim, grant and permit follow that ADR's migration.
- The OpenBao store gap blocks any connection material until the secrets service
  exists; work that deepens the gateway's OpenBao coupling should be weighed
  against that.
- The test harness is an obligation, not an extra:
  - a fake OAuth authorization server that enforces PKCE `S256`, rotates refresh
    tokens with reuse detection, serves revocation, and can return
    `invalid_grant`, `invalid_client`, a reduced scope set, throttling, and a
    dropped response after the request is accepted (the `UNKNOWN` outcome);
  - a fake MCP server and authorization server covering the challenge header,
    path-form and root protected resource metadata, issuer path insertion, a
    Client ID Metadata Document, registration and its deletion, and
    `insufficient_scope`;
  - a local HTTPS callback and a metadata document URL reachable under test;
  - raw-material canaries: every run seeds the fakes with random sentinel values
    that look like nothing real, then scans every stream capture, KV value,
    snapshot, idempotency record, log, trace, metric label and error body for them
    and fails on any hit, in addition to the field-name guard of Decision 8;
  - identity cases: a different account forces a new connection, concurrent
    connects to one account yield one claim, and a claim is never stolen.

## References

- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0032: Model Route and Credential Binding](./0032-model-route-and-credential-binding.md)
- [ADR#0033: Key Custody Product Model](./0033-key-custody-product-model.md)
- [ADR#0045: Event-Sourced Service Module Layout](./0045-event-sourced-service-module-layout.md)
- [ADR#0046: Project-Anchored Resource Hierarchy](./0046-project-anchored-resource-hierarchy.md)
- [ADR#0049: Revocation Propagation Latency Target](./0049-revocation-latency-target.md)
- [ADR#0057: Decider Command NATS Binding](./0057-decider-command-nats-binding.md)
- [ADR#0063: Agent Connection Declarations](./0063-agent-connection-declarations.md)
- [ADR#0066: Project Resource and Members-Only Access](./0066-project-resource-and-members-only-access.md)
- [ADR#0071: Policy Bindings on a Subject](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
- [ADR#0072: Agents Improve through Recorded Candidates](./0072-agents-improve-through-recorded-candidates.md)
- [ADR#0074: Credential Platform Extraction Boundary](./0074-credential-platform-extraction-boundary.md)
- [ADR#0079: External Secret Sources](./0079-external-secret-sources.md)
- [ADR#0083: Sealed Content Envelope](./0083-sealed-content-envelope.md)
- [ADR#0093: Connection Ownership and Delegation](./0093-connection-ownership-and-delegation.md)
- [ADR#0094: OAuth Authorization Protocol](./0094-oauth-authorization-protocol.md)
- [ADR#0095: Connection Credential Custody and Refresh](./0095-connection-credential-custody-and-refresh.md)
- [ADR#0096: Connection Use by Agents](./0096-connection-use-by-agents.md)
- [Provider Credential Custody](../architecture/provider-credential-custody.md)
- [Credential binding](../glossary/credentialbinding), [model provider connection](../glossary/modelproviderconnection), [customer managed key](../glossary/customer-managed-key)
- MCP authorization, version 2026-07-28: https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization
- RFC 7009 (revocation), RFC 7591 and RFC 7592 (dynamic client registration), RFC 8414 (authorization server metadata), RFC 8707 (resource indicators), RFC 9728 (protected resource metadata)

## Operational defaults

The [connector security profile](../reference/connector-security-profile.md)
fixes project-only initial ownership, owner-initiated upgrades, operator approval
and emergency blocking, profile minimization and isolated custody callers.
Keep the package-qualified CredentialBinding name. Linear inbound migration waits
for the separate webhook contract; transparent proxying remains a separate
profile, not an unresolved part of the selected HTTPS relay.
