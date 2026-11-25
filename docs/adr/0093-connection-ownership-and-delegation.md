---
number: "0093"
slug: connection-ownership-and-delegation
status: draft
date: 2026-11-02
---

# ADR#0093: A Connection's Custody Project Is Separate from Its Logical Owner, Person-Owned Use Needs an Explicit Revocable Delegation, and Every Owner Kind Has an Offboarding Cascade

## Context

[ADR#0092](./0092-third-party-connection-model.md) gives a Connection three facts
that are easy to confuse: where its stream and secrets live, who the connection
belongs to, and which provider account consented. Only the first has settled
vocabulary. [ADR#0046](./0046-project-anchored-resource-hierarchy.md) makes the
project the owner boundary: OpenBao paths, stream routing and ids carry the
project id and nothing above it, and a credential is born in a project and dies
there. [ADR#0066](./0066-project-resource-and-members-only-access.md) makes a
project a resource that is never a hierarchy node, keeps membership and access
in the policy plane, and records the creator as envelope provenance
([ADR#0069](./0069-event-envelope-records-issuer-and-times.md)), not as an owner.

Three shapes of connection cannot be told apart by project alone: one a team
shares, one a whole workspace shares, and one that is a single person's consent
to a provider. "Workspace" here is the hierarchy node of [ADR#0066](0066-project-resource-and-members-only-access.md) Decision 2, not
the repository of the [workspace](../glossary/workspace) glossary entry. If custody
were read as authorization, any member of the custody project could use another
member's mailbox. Nothing yet says what becomes of a connection when its owner
leaves, nor where a workspace-wide connection lives, and [ADR#0046](0046-project-anchored-resource-hierarchy.md)'s Consequences
say "workspace-shaped fields collapse into the project".

Offboarding is also an erasure problem. Provider tokens outlive the people who
consented to them, and the provider account's display data is personal data.
[ADR#0083](./0083-sealed-content-envelope.md) Decision 8 shreds at tenant
granularity only and defers per-session erasure,
[ADR#0035](./0035-session-store-decider-aggregate.md) Decision 7 keeps logs
forever, and [ADR#0083](0083-sealed-content-envelope.md) Decision 1 seals sensitive streams for customer managed
tenants alone. A personal fact written into an event can therefore not be erased
for one person while the tenant lives.

## Decision

### 1. Custody, logical owner and provider subject are three separate immutable facts

| Fact | Carried by | Decides |
| --- | --- | --- |
| Custody project | `ConnectionRef.resource_parent.project_id` | Where the stream, the OpenBao path and the quota live, and who operates them |
| Logical owner | `ConnectionOwner` | Who may administer and delegate the connection, and whose departure ends it |
| Provider subject | The account claim of [ADR#0092](0092-third-party-connection-model.md) | Which provider account consented |

```proto
message ConnectionOwner {
  oneof owner {
    option (buf.validate.oneof).required = true;
    ProjectOwner project = 1;      // the custody project is the owner
    WorkspaceOwner workspace = 2;  // custody is the workspace's integration project
    PersonOwner person = 3;        // used only through a delegation
  }
}
```

All three are fixed at creation. A connection never moves between projects
([ADR#0046](0046-project-anchored-resource-hierarchy.md)), and ownership is never transferred: a different owner means a new
authorization attempt ([ADR#0094](./0094-oauth-authorization-protocol.md)), because
the provider's consent belongs to whoever gave it. `resource_parent.project_id` is custody, not
authorization evidence. Admission asks the policy plane about the owner and the
consumer, never whether the caller sits in the custody project.

Resource containment is the typed `resource_parent`, independently recorded in
commands, genesis, state and references. Workspace/person ownership does not
change it. The [resource-parent contract](../reference/connector-resource-parents.md)
also defines parent-scoped public resource names and authenticated equality checks.

### 2. A project-owned connection is administered through the project and used through a permit

The owner is the custody project. Connecting, reconnecting and disconnecting
follow project membership in the policy plane ([ADR#0066](0066-project-resource-and-members-only-access.md) Decision 1). The creator
is provenance, so a creator who leaves changes nothing about the connection.
Membership is not a use right: an Agent uses the connection only through a permit
(Decision 5).

### 3. A workspace-owned connection lives in the workspace's integration project

A workspace is a bare node that owns nothing ([ADR#0066](0066-project-resource-and-members-only-access.md) Decision 1), and storage
anchors at a project ([ADR#0046](0046-project-anchored-resource-hierarchy.md) Decision 2), so a workspace-owned connection needs
a home. A workspace designates one integration project: an ordinary project at or
below the workspace node, chosen by the workspace's administrators. The
designation is a policy binding on the workspace node naming one exact `ProjectId`,
checked at admission the way [ADR#0066](0066-project-resource-and-members-only-access.md) Decision 2 checks `parent`. `WorkspaceOwner`
carries the workspace's `NodeId`, and creation is refused unless the custody
project is the current integration project. Redesignating moves nothing, because
the designation governs creation only.

Admission also checks on every use that the custody project still sits at or
below the owner workspace. A hierarchy move that breaks this makes the connection
unusable, never re-owned, and raises a fault that an administrator resolves by
moving the project back or disconnecting.

This amends [ADR#0046](0046-project-anchored-resource-hierarchy.md). Its Consequences line "workspace-shaped fields collapse
into the project" stays true of custody, which still collapses into the project,
and stops being true of ownership, which is logical and may name a workspace.

### 4. A person-owned connection is used only through an explicit, revocable delegation

A person-owned connection is one person's consent to a provider. The custody
project is the project where the person ran the flow, since there is no vault
outside a project. `PersonOwner` carries the stable principal identifier that
[ADR#0069](0069-event-envelope-records-issuer-and-times.md) records for an actor. Delegation here means connection delegation, the
lending of one person's authority over one connection; it is not the session
delegation of the [delegation](../glossary/delegation) glossary entry.

- **Custody confers no use.** Project membership, and any target that names a
  project, never reaches a person-owned connection.
- **Use needs a delegation, and a delegation is a permit:** a `ConnectionPermit`
  naming this connection's exact `ConnectionId`. There is no separate delegation
  record. Only the owner's own authenticated act can create it (the envelope
  actor is the owner), and the owner can revoke it at any time. A tenant
  administrator ends a delegation by revoking it, never by holding it, because
  holds bind only to a `PolicySubject`, an `AgentId` or a `NodeId`
  ([ADR#0071](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
  Decision 4), and a tenant administrator can never create one. A hold on the
  delegate Agent still stops that Agent's Sessions.
- **A delegation names an exact `AgentId`, never a node,** and carries
  authorized HTTP proxy use only, optionally narrowed by the permit's `ceiling`
  (Decision 5). Permits do not follow delegation lineage ([ADR#0071](0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
  Decision 3), so an Agent the delegate dispatches to receives no part of it
  unless the owner names that Agent too.
- **Liveness follows access.** A person-owned connection is live only while its
  owner can access the custody project, and admission re-evaluates this on each
  use (Decision 9).

A delegated call is attributed to both the Agent and the owning person;
[ADR#0096](./0096-connection-use-by-agents.md) owns that record.

### 5. `policy/connection_permissions` grants use by exact connection or by custody project

The module takes the shape [ADR#0071](0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md) Decision 6 gives `policy/delegation_permissions`.
A `ConnectionPermissionPolicy` binds to a `PolicySubject`, absence is denial, an
`Initialize` command installs the Agent's connection permits at provisioning, as
[ADR#0071](0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md) Decision 6 prescribes, not an empty policy, so an Agent with a `REQUIRED`
connection declaration ([ADR#0063](./0063-agent-connection-declarations.md)) does
not fail its first Session. `Initialize` installs only permits the provisioning
actor may write on the owner side, so it never installs a delegation. Each rule
records a `PolicyRuleRef`, and the selector resolves first and the permission
second. Policy may refuse a resolution but
never steers it.

```proto
message ConnectionPermit {
  ConnectionTarget target = 1;
  repeated ConnectionUseMode modes = 2;  // empty means HTTP_PROXY only
  ConnectionCeiling ceiling = 3;         // enforced request restrictions, scopes, MCP tool allowlist
}
message ConnectionTarget {
  oneof target {
    ConnectionId connection_id = 1;      // any owner kind
    ConnectionsUnder under = 2;          // one custody project; never person-owned
  }
}
```

| Target | Project-owned | Workspace-owned | Person-owned |
| --- | --- | --- | --- |
| Exact `connection_id` | Permitted | Permitted | Permitted as a delegation, on an exact `AgentId` |
| Connections under a custody project | Permitted | Permitted (those in that integration project) | Never matches |

The optional `ceiling` narrows a permit to restrictions the proxy can enforce
on an HTTP request, and for an MCP connection to a tool allowlist. Provider-granted
scopes remain an upper bound. HTTP method and path alone cannot distinguish every
provider action, so the proxy must reject a restriction it cannot enforce rather
than claim provider-specific read or write isolation. A delegation can lend less
than the person holds only through enforced restrictions.

Authorized HTTP proxy use is the default: the proxy makes the call and the Agent
never sees a provider token. Optional future token issuance must be named
explicitly and is refused for person-owned connections, because a raw token
handed out outlives the delegation that issued it. It is not required for proxy
calls. [ADR#0096](./0096-connection-use-by-agents.md) defines that boundary.

The protobuf modes are `HTTP_PROXY`, `MCP_SESSION` and optional future
`TOKEN_ISSUANCE`; the retired `INVOKE_OPERATION` number and name are reserved.
A scope ceiling is an allowlist of actual credential scopes, never a mapping
from HTTP requests to provider actions. HTTP restrictions match host, method
and path prefix; mode-inapplicable or unsupported restrictions are refused.
A grant contains a proxy binding only for HTTP proxy use and has a matching
audience. The [relay reference](../reference/connection-http-proxy.md) defines
request authentication and how the proxy composes these restrictions.

A permit is written against the owner side. For a project or workspace owner the
actor must administer that owner in the policy plane, and for a person owner the
actor must be that person. The actor must also be able to bind policy to the
subject ([ADR#0071](0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md) Decision 1), so whoever administers a consumer cannot widen a
connection they do not administer. A Session records what it evaluated ([ADR#0071](0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
Decision 7).

Evaluation is bounded-live. Admission, the connection proxy and the trusted MCP
executor evaluate permits, standing and connection state from a projection whose
staleness is measured from a heartbeat. They refuse once that staleness passes a
configured bound of at most 330 seconds (the cache TTL plus jitter of
[ADR#0049](./0049-revocation-latency-target.md)) and never fall back to the last
projection held. This matches the bounded-live rule proposed for [ADR#0071](0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
in PR #751 (in review). Revocation latency is [ADR#0049](0049-revocation-latency-target.md).

### 6. The provider app resolves from custody project, then platform

An authorization attempt needs a provider app ([ADR#0092](0092-third-party-connection-model.md)). Eligible apps registered
in the custody project take precedence over platform-shared apps. Two eligible
apps at the selected scope fail with an ambiguity fault, following the rule of
[ADR#0063](0063-agent-connection-declarations.md) Decision 4. Policy may refuse the resolved app and never changes which
one resolves. The logical owner kind does not change this selection: a person or
workspace owner does not supply another app namespace.

`ProviderAppParent` supports project and platform containers. A project app must
share the Connection and authorization attempt's custody parent. Node-scoped app
registration and borrowing an app from another project are deferred; hierarchy
placement alone does not authorize either. Adding them requires an explicit
parent and sharing contract rather than treating a ProjectId as a NodeId.

Disconnecting or cascading a connection removes the connection's reference to its
app and nothing else. It never deletes or revokes the app, including a
platform-shared app. App retirement belongs to [ADR#0092](0092-third-party-connection-model.md).

### 7. Nothing is shared across tenants

A connection, a permit, a delegation, an integration designation and an app
reference never name anything in another tenant. The tenant is the isolation
boundary ([ADR#0046](0046-project-anchored-resource-hierarchy.md) Decision 5) and the NATS account is where it is enforced
([ADR#0083](0083-sealed-content-envelope.md) Context). Admission rejects a permit whose subject and target are in
different tenants. A platform-shared app is a platform resource that tenants
reference; the connection, tokens and profile it yields belong to one tenant. Two
tenants that want the same provider account each run their own flow.

### 8. Phase 1 enables the project arm only

| Phase | Owner arms | Permits | Cascades |
| --- | --- | --- | --- |
| 1 | Project | Exact id or under-project, subject an Agent in the custody project | Project archived (a standing join), tenant deleted |
| 2 | Adds workspace | Integration designation, cross-project permits authorized on the owner side | Adds workspace deleted |
| 3 | Adds person | Delegation | Adds person leaves, member removed |

The schema carries all three arms from its first version ([ADR#0092](0092-third-party-connection-model.md) Decision 4).
Policy rejects an arm that is not enabled at creation with a typed denial, so
enabling one is a policy change and needs no wire change. In Phase 1 a member's
removal touches no connection, because project-owned connections survive their
creator and members.

### 9. Offboarding cascades run in reactors outside the aggregates

Each cascade reacts to a fact another plane owns, which the Connection decider
cannot observe: its fold reads its own stream ([ADR#0045](0045-event-sourced-service-module-layout.md) gives each stream one
module, and [ADR#0066](0066-project-resource-and-members-only-access.md) Decision 2 keeps the Project fold off hierarchy facts). A
cascade is therefore a reactor: a durable consumer of the trigger fact that issues
ordinary commands to the Connection aggregate. It runs in the connections service
beside the cleanup worker the secrets service already has
([ADR#0023](./0023-secret-management-and-key-custody-direction.md) recovery worker,
[ADR#0074](./0074-credential-platform-extraction-boundary.md) Decision 3,
`lifecycle_worker_cleanup`).

- **At-least-once and idempotent.** The consumer acknowledges only after the
  commands are accepted. Each command id derives from the trigger event id, the
  connection and the step, so redelivery collapses
  ([ADR#0057](./0057-decider-command-nats-binding.md) `command_id`).
- **A named actor.** The reactor acts under a service identity recorded as the
  envelope actor ([ADR#0069](0069-event-envelope-records-issuer-and-times.md)), and the revoke command names the trigger.
- **Order for one connection.** (1) The revoke intent commits, so admission denies
  at once and caches evict within the [ADR#0049](0049-revocation-latency-target.md) bound. (2) Refresh stops at the
  terminal state ([ADR#0095](./0095-connection-credential-custody-and-refresh.md)).
  (3) Permits and delegations naming the connection are removed, and grants
  already admitted lapse within their short lifetime ([ADR#0096](0096-connection-use-by-agents.md)). (4) The provider
  is asked to revoke, with each outcome recorded as an `EffectOutcome`
  ([ADR#0094](0094-oauth-authorization-protocol.md) Decision 13). (5) Material is destroyed once the revoke outcome is
  terminal or its window ends ([ADR#0095](0095-connection-credential-custody-and-refresh.md)). (6) The profile is purged (Decision 11).
- **Cleanup debt.** A step that cannot finish leaves a durable, queryable and aged
  open obligation on the connection: provider revoke residuals ([ADR#0094](0094-oauth-authorization-protocol.md)),
  undestroyed material ([ADR#0095](0095-connection-credential-custody-and-refresh.md)), an unpurged profile (this ADR). Every step ends
  as a completed fact or open debt, never silently dropped, and a trigger is not
  done while debt for it is open.
- **Races.** Creating a connection or permit checks that the owner is live and
  can access the custody project, and after a trigger the reactor lists the
  owner's connections again, so one created during the race is caught. Admission
  also checks owner liveness on each use under the bounded-live rule of Decision 5,
  so a lagging reactor costs no more than that bound for use. It does not shorten the life of a token the provider
  already issued.

### 10. Each owner kind has a trigger, a reactor and a defined end

| Trigger and fact | Connections | Grants, refresh, provider revoke, debt |
| --- | --- | --- |
| Person leaves the tenant: the identity plane deactivates the principal | Every person-owned connection of that person is revoked, terminally. Project- and workspace-owned connections the person created are untouched ([ADR#0066](0066-project-resource-and-members-only-access.md) Decision 5) | All the person's delegations end; grants lapse; refresh stops at the revoke. Provider revoke runs and the profile is purged; failures become debt |
| Member removed from a project or workspace: the policy plane removes the membership | The reactor re-evaluates person-owned connections whose owner lost access to the custody project and revokes those. Project- and workspace-owned connections are untouched | As above for the revoked ones; the person's delegations from other projects die with the connection |
| Project archived: `ProjectArchived` | No per-connection event is written. Admission, the connection proxy, the trusted MCP executor and the refresh scheduler read project standing live, as release and app standing are read, so every connection with custody in the project, of any owner kind, is refused and refresh pauses. `ProjectUnarchived` lifts the refusal. A workspace-owned connection in an archived integration project is refused for every consumer | Permits stay dormant, grants lapse, no provider call is made. A refresh token that went idle during the archive makes the connection ReconnectRequired once refresh resumes. After the archive window the connection is revoked as a disconnect (connector security profile) |
| Workspace deleted: the hierarchy plane deletes the node | Workspace-owned connections are revoked and the integration designation ends. Projects below survive by their own facts, and person-owned connections in them follow the row above | Permits naming the revoked connections are removed, then Decision 9 order applies |
| Tenant deleted: tenant lifecycle starts deletion | Every connection of every owner kind is revoked | Every permit and delegation is removed, every profile purged, then the ordering of Decision 12 applies |

An Agent that is archived loses its permits with its subject in the policy plane,
and no connection is affected. The facts named in the first column belong to
other planes, and this ADR names the contract, not the events.

### 11. Personal data about the provider account lives in an erasable profile record

An event cannot be erased for one person (Decision 12), so the Connection stream
holds none of it ([ADR#0092](0092-third-party-connection-model.md) Decision 9). A
provider account profile is a mutable KV record keyed by tenant, custody project and
`ConnectionId` holding the display name, the raw provider ids of a user subject
and nothing else personal. The callback path
([ADR#0094](0094-oauth-authorization-protocol.md)) writes it before the activation command, and the stream references it
by a keyed digest of the account identity, in the manner of [ADR#0083](0083-sealed-content-envelope.md) Decision 6.
The digest is the claim key and the equality check for a same-account reconnect.
Streams, claims, projections, audit and metrics hold the digest and the opaque
`ConnectionId`, never the profile. A profile written for an attempt that never
activates is an orphan, and cleanup debt purges it.

### 12. Erasure is a purge of the profile, or a shred of the tenant, and never a rewrite of a stream

| Scope | Mechanism | Result |
| --- | --- | --- |
| One connection's profile (offboarding, disconnect, erasure request) | Purge the KV record with its revisions; the stream gains an erasure fact carrying the digest and the time, no content | Personal data gone; connection history intact |
| A tenant | Cascade and its debt close, then key destruction ([ADR#0083](0083-sealed-content-envelope.md) Decision 8) for sealed series and an operator purge by subject for any unsealed series (the model of [ADR#0029](./0029-decider-retention-and-truncation-watermark.md)) | Every series of the tenant is unreadable or removed |
| One person inside a stream | No mechanism. Per-person keys multiply registry entries, leases and provider calls, which is why [ADR#0083](0083-sealed-content-envelope.md) defers even per-session keys | Pseudonymous ids remain |

The Connection streams are sensitive streams ([ADR#0092](0092-third-party-connection-model.md) Decision 8), sealed for
customer managed tenants and held in plaintext under the server cipher for others
([ADR#0083](0083-sealed-content-envelope.md) Decision 1). Crypto-shredding therefore erases a customer managed
tenant's connection history by itself and nothing for another tenant, whose series
the operator purge removes. History from before a series' sealing cutover is
outside key destruction until a rebuild re-seals it ([ADR#0083](0083-sealed-content-envelope.md) Decision 5), and the
purge covers it. A read-time redaction mask in the style of [ADR#0035](0035-session-store-decider-aggregate.md) Decision 7
hides content and removes no bytes, so it is not erasure.

**Tenant deletion is ordered.** (1) The business audit record of [ADR#0023](0023-secret-management-and-key-custody-direction.md) Decision 6
is written and the NATS account is disabled, not deleted
([ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md)). (2) The
cascade of Decision 9 runs for every connection. (3) Cleanup debt is closed or
abandoned with a recorded outcome, bounded by the revoke window of [ADR#0094](0094-oauth-authorization-protocol.md). (4)
Profiles are purged. (5) Keys are destroyed and unsealed series purged. (6) The
completion audit record is written. Key destruction comes last, and for a customer managed key it waits for the
revoke-all that [ADR#0095](0095-connection-credential-custody-and-refresh.md) raises as an open question, since destroying
the key first would make connection state unreadable and strand provider-side
tokens with no way left to revoke them.

**Retention.** Audit records carry opaque ids, owner kind, trigger, outcome and
time, never free text or profile data
([ADR#0078](./0078-api-key-rotation-grace-and-audit-set.md)). They live outside the
tenant's sealed content and survive its key destruction for the platform audit
retention, which this ADR does not set. A disconnected connection's stream follows
stream retention ([ADR#0029](0029-decider-retention-and-truncation-watermark.md)) and, with its profile purged, holds only pseudonymous
ids and the owner's label. A restore from backup re-applies the revocations and
purges recorded after the backup point before any connection becomes usable;
mechanics are [ADR#0095](0095-connection-credential-custody-and-refresh.md).

## Alternatives Considered

**Custody at the workspace node.** Rejected: nodes are bare identifiers
([ADR#0066](0066-project-resource-and-members-only-access.md)) and storage anchors at the project ([ADR#0046](0046-project-anchored-resource-hierarchy.md) Decision 2). A node-held
vault would be the parallel workspace concept [ADR#0046](0046-project-anchored-resource-hierarchy.md) refused.

**Derive ownership from custody.** Rejected: one person's consent would be usable by
every project member, and offboarding could not tell which connections to revoke.

**Let a subtree permit cover person-owned connections.** Rejected: one rule would
widen a personal grant to every present and future Agent below a node. An exact id
leaves the record of who widened what, as [ADR#0071](0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md) Decision 6 argues.

**Move or transfer connections between projects or owners.** Rejected: [ADR#0046](0046-project-anchored-resource-hierarchy.md)
disallows cross-project moves, and a transfer would not transfer the consent the
provider recorded.

**Share connections across tenants.** Rejected: it breaks the account boundary and
leaves a tenant deletion unable to cascade through a connection others use.

**Offboard inside the aggregate, or lazily at use time only.** Rejected: an
aggregate cannot see other streams, and a lazy check leaves refresh and
provider-held tokens alive. The use-time check stays as defense in depth.

**Keep display data in the sealed stream and erase with the tenant only.** Rejected:
it cannot honor one person's erasure in a live tenant, and for a tenant without a
customer managed key the stream is not sealed at all.

**Per-person sealing keys, or truncating a stream to erase.** Rejected: the first
multiplies registry entries ([ADR#0083](0083-sealed-content-envelope.md) Decision 8), the second breaks the immutable
log ([ADR#0035](0035-session-store-decider-aggregate.md) Decision 7) and purge stays an operator action ([ADR#0029](0029-decider-retention-and-truncation-watermark.md)).

**Write a Suspended state into every connection when a project is archived.**
Rejected: it fans one archive out into a write per connection and keeps a second
copy of project standing. `Suspended` stays the owner's or operator's reversible
choice for one connection.

**Place a hold on a connection or a delegation.** Rejected: [ADR#0071](0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md) holds bind
only to a `PolicySubject`, so a delegation ends by revocation.

**Archive revokes immediately.** Rejected: `ProjectUnarchived` exists, and a
reversible event should not destroy consent that only the provider's account holder
can renew.

## Non-Goals

- Provisioning the identity lifecycle: which system deactivates a principal.
- What deleting a workspace that still has projects means to the hierarchy.
- Revoke mechanics ([ADR#0094](0094-oauth-authorization-protocol.md)), token storage, destruction, retention and restore
  ([ADR#0095](0095-connection-credential-custody-and-refresh.md)), runtime admission and call attribution ([ADR#0096](0096-connection-use-by-agents.md)), and the app
  catalog ([ADR#0092](0092-third-party-connection-model.md)).
- Regulatory retention periods and legal hold.
- Ownership transfer, cross-tenant sharing and per-person crypto-shredding.

## Consequences

- [ADR#0046](0046-project-anchored-resource-hierarchy.md) is amended: custody stays project-anchored, and a workspace may own a
  connection whose custody is its integration project.
- [ADR#0071](0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md) gains `policy/connection_permissions` in the shape of Decision 6, with
  one case Decision 6 lacks: a permit only the owner's own act can create.
- Other planes owe durable facts for principal deactivated, membership removed,
  workspace deleted and tenant deletion started. `ProjectArchived` exists.
- Cleanup debt is a new durable obligation with a queryable, aged surface.
- Tenant deletion is ordered and not instant: key destruction waits for the revoke
  window.
- A person who loses access to a custody project loses the person-owned
  connections there, even while remaining in the tenant, and reconnects in a
  project they can access.
- Phase 1 ships the project arm, the schema's other arms, the project standing
  join and the tenant-deletion reactor.
- The glossary needs entries for connection delegation, custody project,
  integration project, provider account profile and cleanup debt.

## References

- [ADR#0023: Secret Management and Key Custody Direction](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0029: Decider Retention and Truncation Watermark](./0029-decider-retention-and-truncation-watermark.md)
- [ADR#0035: Session Store Decider Aggregate](./0035-session-store-decider-aggregate.md)
- [ADR#0045: Event-Sourced Service Module Layout](./0045-event-sourced-service-module-layout.md)
- [ADR#0046: Project-Anchored Resource Hierarchy](./0046-project-anchored-resource-hierarchy.md)
- [ADR#0049: Revocation Latency Target](./0049-revocation-latency-target.md)
- [ADR#0057: Decider Command NATS Binding](./0057-decider-command-nats-binding.md)
- [ADR#0063: Agent Connection Declarations](./0063-agent-connection-declarations.md)
- [ADR#0066: Project Resource and Members-Only Access](./0066-project-resource-and-members-only-access.md)
- [ADR#0069: Every Event Envelope Records Who Caused It and When Each Step Happened](./0069-event-envelope-records-issuer-and-times.md)
- [ADR#0071: Policy Bindings on a Subject](./0071-policy-bindings-on-a-subject-entitlements-ceilings-and-holds.md)
- [ADR#0074: Credential Platform Extraction Boundary](./0074-credential-platform-extraction-boundary.md)
- [ADR#0078: API Key Rotation Grace and Audit Set](./0078-api-key-rotation-grace-and-audit-set.md)
- [ADR#0081: NATS TLS by Default and Operator Mode Admission](./0081-nats-tls-by-default-and-operator-mode-admission.md)
- [ADR#0083: Sealed Content Envelope](./0083-sealed-content-envelope.md)
- [ADR#0092: Third-Party Connection Model](./0092-third-party-connection-model.md)
- [ADR#0094: OAuth Authorization Protocol](./0094-oauth-authorization-protocol.md)
- [ADR#0095: Each Connection's OAuth Token Set Is One Sealed, CAS-Written OpenBao Secret, Refreshed Only Through a Write-Ahead Credential Transaction](./0095-connection-credential-custody-and-refresh.md)
- [ADR#0096: Connection Use by Agents](./0096-connection-use-by-agents.md)

## Operational defaults

The [connector security profile](../reference/connector-security-profile.md)
fixes project-only rollout, archive cleanup, future delegation expiry, exact-Agent
personal delegation and audit retention. Identity, policy, hierarchy and tenant
lifecycle planes emit principal, membership, workspace and tenant offboarding
facts respectively, with durable event identities. Hierarchy deletion refuses a
workspace that still contains projects until those projects are moved or archived.
Workspace integration-project selection is a policy binding governing creation.
The canonical account fingerprint remains in the stream; bounded labels remain
subject to the metadata privacy policy. Key rotation follows the
[custody identity contract](../reference/connector-account-identity.md).
