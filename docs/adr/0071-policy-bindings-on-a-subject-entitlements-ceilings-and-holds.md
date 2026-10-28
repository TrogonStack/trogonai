---
number: "0071"
slug: policy-bindings-on-a-subject-entitlements-ceilings-and-holds
status: draft
date: 2026-10-18
---

# ADR#0071: What an Agent May Reach, How Far It May Go, and Whether It May Run at All Are Policy Bindings on a Subject

## Context

[ADR#0025](./0025-agent-definition-data-ownership.md) settles that an Agent's
configuration declares what the Agent needs and that external planes decide
what it may have. The declaration side is complete:
[ADR#0062](./0062-runtime-owned-settings-and-platform-declarations.md) and
[ADR#0063](./0063-agent-connection-declarations.md) give a revision its skill,
tool, delegate, memory, and connection declarations, and
[ADR#0070](./0070-agent-offered-interface-and-throughput-limits.md) adds what
the Agent offers and whose authority it carries. The deciding side is not. The
policy plane holds money ceilings in `policy/agent_budgets` and throughput
ceilings in `policy/agent_rate_limits`, and nothing else.

That leaves gaps that an operator meets on the first day:

- **Nothing authorizes a delegation.** A `DelegateDeclaration` grants no
  authority, by design, and no record anywhere grants it either. A Session that
  resolves a delegate today is admitted on the strength of the declaration
  alone, which is the substitution of a declaration for a grant that
  [ADR#0025](./0025-agent-definition-data-ownership.md) forbids.
- **Nothing caps which models an Agent may select.** A connection's allowed
  models under [ADR#0032](./0032-model-route-and-credential-binding.md)
  authorize what the credential may reach, and every Agent sharing that
  connection reaches all of it. An operator who wants one Agent on a cheap
  model and another on a frontier model has nowhere to say so.
- **Nothing stops an Agent short of archiving it.** Archiving blocks new work
  but also blocks new revisions, so a held Agent cannot be fixed forward. A
  zero rate limit stops only model requests. Both change records the author
  owns, while the person stopping the Agent is usually someone else: an
  on-call responder or a security reviewer who should never need write access
  to the registry.

[ADR#0070](./0070-agent-offered-interface-and-throughput-limits.md) deferred a
paused lifecycle state and guardrail bindings. This record answers the first
by a different mechanism and keeps the second deferred with a trigger.

The [agent platform research index](../research/agent-platform/index.md)
records the products whose entitlement, group, and pause features were studied
before this record was written. None of them is cited as a precedent; each
binds these controls onto the agent record, which is the shape
[ADR#0025](./0025-agent-definition-data-ownership.md) rules out.

## Decision

### 1. A policy binding attaches to a subject, and a subject is an exact identity

Every policy binding introduced here attaches to a
`trogonai.policy.subjects.v1alpha1.PolicySubject`: one Agent by `AgentId`, or
one `trogon.hierarchy.v1alpha1.NodeId` so that every Agent placed at or below
the node is covered. Binding at a node is how a project, a workspace, or a
tenant is frozen or capped in one command.

Subjects and targets are exact identities. A binding never names an Agent by
display name, label, or pattern, because a relabel would then move authority
onto a different Agent without any policy change, which is the escalation
[ADR#0025](./0025-agent-definition-data-ownership.md) property 1 exists to
prevent, and because [ADR#0040](./0040-contract-field-vocabulary.md) keeps
late-bound names for the one place that wants late binding. `AgentId` is a
value object in the agents package per
[ADR#0068](./0068-reference-fields-say-they-are-references.md). Older
messages that carry the identifier as a bare string keep doing so; a wire
rename is not worth a breaking change.

### 2. Permits widen downward, ceilings and holds bound downward, and a denial is always one rule

[ADR#0066](./0066-project-resource-and-members-only-access.md) Decision 3
gives the hierarchy its mood: a grant accumulates downward, a limit caps
everything below it. The bindings here take one mood each and never mix them:

- a **permit** widens. The effective permits of an Agent are the union of its
  own and every ancestor node's. Absence is denial.
- a **ceiling** bounds. Every ceiling on the chain must admit the action.
  Absence is no ceiling from that subject, never a zero cap.
- a **hold** bounds. Any hold on the chain refuses the action.

An action is admitted when some permit covers it, every ceiling admits it, and
no hold refuses it. There are no deny lists and no precedence rules between
bindings of the same kind, so every refusal is explainable by naming the one
binding that caused it: the permit that was missing, the ceiling that
excluded, or the hold that was placed.

### 3. Ceilings and holds follow the delegation lineage; permits do not

A Session's lineage is the chain of Agents whose Sessions dispatched one
another, from the root Session down, together with each Agent's ancestor
nodes. It is the same chain `policy/agent_budgets` already retains as
authorization provenance, and it is the chain
[AAuth](../glossary/aauth) carries as nested actor claims when a call leaves
the platform under [ADR#0017](./0017-aauth-agent-authentication.md). Repeated
identities in a lineage count once, and detaching a child does not remove the
scopes it inherited at dispatch.

Ceilings and holds evaluate over the whole lineage. An Agent limited to one
model cannot reach another by delegating to an Agent that may use it, and a
hold on an Agent stops the work it caused, not only the work it runs itself.

Permits evaluate on the dispatching Agent's own chain only. A permit letting
Agent A reach Agent B says nothing about what B may reach; B's own permits
decide that when B dispatches. A child Session therefore runs under the
callee's own entitlements, bounded by the caller's lineage, which is the
service-account model rather than the impersonation model.

### 4. Holds replace a paused lifecycle state

`policy/holds` is a bound-downward stop placed on a subject without touching
the registry lifecycle. A hold refuses; it does not cancel, and interrupting
work that is already running stays with the session plane's `CancelSession`.

Holds are named. `HoldName` is a coordination handle chosen by the party
placing the hold, resolved by `ReleaseHold`, so it is a name in the
[ADR#0040](./0040-contract-field-vocabulary.md) sense. Independent parties
hold the same subject at once under different names, and releasing a security
hold never releases an owner's hold.

A hold has a scope:

- `NEW_WORK` refuses new Sessions of the subject and new delegations into it.
  Running Sessions finish on their own terms.
- `ALL_PROTECTED_ACTIONS` refuses the same, and every running Session under
  the hold fails closed at its next protected action: a model request, a tool
  call, or a delegation dispatch. Nothing is suspended for later resumption.
  A suspended Session would hold a checkout, a reservation, and a provider
  stream open for an unbounded time, and the hold would then have to record
  enough to resume safely, which is the session plane's job under
  [ADR#0035](./0035-session-store-decider-aggregate.md), not the policy
  plane's.

There is no `Initialize` command and no provisioning participation. A hold
needs nothing set up before it applies, and it must be placeable on an Agent
whose provisioning is still pending. The first `PlaceHold` creates the
subject's hold history. There is no reason text: the stream is immutable and
free text invites personal data, and the actor who placed or released a hold
comes from the event envelope under
[ADR#0069](./0069-event-envelope-records-issuer-and-times.md).

The lifecycle was the wrong place because of who owns it and what moves with
it. Lifecycle belongs to the author; a hold belongs to whoever is responding.
Changing the lifecycle mid-incident would also rebind the dispatch gates of
every policy module that keys on it, so a budget could become unsettable at
the moment it most needs changing.

### 5. Model ceilings narrow what a connection already authorizes

`policy/model_ceilings` binds a `ModelCeilingPolicy` to a subject: the set of
catalog models, matched by exact `model_id` as carried on `ModelVersionRef`,
within which every model selection of the subject's Sessions must fall,
auxiliary roles included.

This is a ceiling, not an entitlement, because a model grant already exists.
[ADR#0032](./0032-model-route-and-credential-binding.md) authorizes models
through a connection's allowed models, and a per-Agent grant would hold a
second copy of that authorization free to drift from the first. A ceiling
only narrows. Absence means no ceiling from that subject, never a zero cap,
and never a way around the connection's own list.

Policy never substitutes. A selection outside the ceiling fails admission
under the no-substitution rule of
[ADR#0032](./0032-model-route-and-credential-binding.md) Decision 6, and
nothing picks a permitted model on the Session's behalf. The matcher has one
arm today; a version arm is an additive change for a ceiling that needs to pin
releases. Families, names, and patterns are not arms, because a late-bound
name would widen a ceiling with no policy change.

### 6. Delegation permissions are the grant, and absence is denial

`policy/delegation_permissions` binds a `DelegationPermissionPolicy` to a
subject: the targets whose Sessions it may dispatch, each an exact `AgentId`
or `agents_under` a hierarchy node. A dispatch is admitted when the resolved
target is named exactly or is placed at or below a permitted node.

Absence is denial from the first day. An Agent with no permission on itself or
any ancestor node may not delegate, and its `DelegateDeclaration` grants
nothing, which is the separation of grant from declaration that
[ADR#0063](./0063-agent-connection-declarations.md) Decision 5 already makes
for connections. A default that let an Agent reach anything in its own project
would be a grant derived from a declaration, with no record of who made it.
A team that wants that shape sets a permission at the project node, and the
record then says who widened what. Grandfathering never derives from
declarations.

Because absence is denial, the module has an `Initialize` command with the
same provisioning role as `policy/agent_budgets`. An Agent provisioned with a
`REQUIRED` delegate declaration would otherwise fail its first Session, since
nothing could have installed its permission before it existed.

The selector resolves first and the permission is checked second. A
`DelegateSelector` resolves over the Session's hierarchy scope, ambiguity
fails there as [ADR#0062](./0062-runtime-owned-settings-and-platform-declarations.md)
already requires, and the permission check then runs on the single resolved
Agent. Policy may refuse a resolution but never changes which Agent it
resolves to, because a selector that policy could steer would be a selector
whose outcome no reviewer could predict from the declaration.

### 7. Evaluation is live, Sessions record what they evaluated, and targets exist before they are named

Every binding is evaluated at session admission and again at each protected
action. Nothing copies a policy into a Session or a plan, in keeping with the
rule that authorization never pins. What a Session records is a
`PolicyRuleRef` for each history it consulted: the kind, the subject, and the
stream revision that was current, with zero meaning the history did not
exist. A past decision is then reconstructable from the policy streams at the
recorded revisions.

A target kind ships only when the resource it names has a contract. Agents
and catalog models have one; tools, tool sources, connections, and guardrails
do not yet. A permission naming a tool today would be an unverifiable string,
the same objection [ADR#0070](./0070-agent-offered-interface-and-throughput-limits.md)
raised against guardrail bindings.

### 8. Rejected or deferred

- **A paused registry lifecycle.** Replaced by holds for the ownership and
  gate-rebinding reasons above.
- **Suspend and resume on hold.** Rejected; a hold fails closed.
- **Deny lists.** Rejected. Denial by conjunction keeps every refusal
  attributable to one binding; a deny list reintroduces precedence.
- **Agent groups as a second placement axis.** Rejected. Hierarchy nodes
  already group subjects, and a second axis would need its own precedence
  rules against the first. A bundle of targets reusable across bindings is a
  different thing, an entitlement set, and is deferred until a second target
  kind exists to bundle.
- **Model families or name patterns as matcher arms.** Deferred until the
  model catalog has a contract for them; exact `model_id` until then.
- **Guardrail bindings.** Deferred, with a trigger: a versioned guardrail
  resource and an enforcement point, which is the model access service or the
  tool boundary. The binding will take the ceiling mood, bind to a subject,
  pin the guardrail version, and name its action; descendants will not opt
  out.
- **Tool, tool source, and connection entitlements.** Deferred until their
  registries exist. The shape is the delegation permission's shape with a
  different target.
- **Kill-switch webhooks, static request headers, agent card signatures, and
  an external identity binding.** These exist only when the platform proxies
  an externally hosted Agent. A kill switch is a subscriber to hold events on
  that Agent's connection record; static headers are forbidden as credentials
  under [ADR#0032](./0032-model-route-and-credential-binding.md); card
  signatures are an export concern of
  [ADR#0012](./0012-ard-compatible-discovery-catalog.md); the identity
  binding is its own resource under
  [ADR#0053](./0053-external-oidc-federation-surface.md) Decision 3. The
  trigger is the first externally hosted Agent admitted.
- **Reusing the name `ModelAccessGrant`.** That name is the attempt-scoped
  token of [ADR#0032](./0032-model-route-and-credential-binding.md), and
  a ceiling is not a grant.

## Consequences

- An Agent with no delegation permission on its chain cannot delegate, and
  existing revisions with delegate declarations stop resolving delegates until
  someone installs a permission. This is the intended break: a delegation
  nobody granted was never authorized.
- Provisioning gains a participant: an operation that provisions an Agent with
  a required delegate declaration must initialize its delegation permission
  before activation, as it already does for a requested budget.
- A hold placed at a node freezes every Agent below it, including Agents
  provisioned after the hold was placed.
- Session admission and each protected action must consult holds, model
  ceilings, and delegation permissions over the lineage rules above, and must
  record a `PolicyRuleRef` for each history consulted. Until enforcement
  lands, every binding here is a reviewable record without effect.
- Model selection failures gain a cause distinct from an unavailable model:
  a ceiling excluded it. The session plane's admission failure contract must
  carry that distinction when it is written, so a reviewer never reads a
  policy refusal as an outage.
- `DispatchExternalDelegation.authorization_reference` is a bare string. It
  must become a typed reference before the first external dispatch is written,
  so that the authorization it names can be a `PolicyRuleRef`.
- `policy/agent_budgets` and `policy/agent_rate_limits` keep their bare string
  `agent_id`. They predate `AgentId`, and a binding keyed by an Agent is the
  Agent-subject arm of a `PolicySubject` in all but spelling.

## References

- [ADR#0012: ARD-Compatible Discovery Catalog](./0012-ard-compatible-discovery-catalog.md)
- [ADR#0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR#0025: Agent Definition Data Ownership](./0025-agent-definition-data-ownership.md)
- [ADR#0032: Model Route and Credential Binding](./0032-model-route-and-credential-binding.md)
- [ADR#0035: Session Store as a Decider Aggregate on NATS JetStream](./0035-session-store-decider-aggregate.md)
- [ADR#0040: Contract Field Vocabulary](./0040-contract-field-vocabulary.md)
- [ADR#0053: External OIDC Federation Surface](./0053-external-oidc-federation-surface.md)
- [ADR#0062: Runtime-Owned Settings and Platform Declarations](./0062-runtime-owned-settings-and-platform-declarations.md)
- [ADR#0063: An Agent Declares the Connections It Needs](./0063-agent-connection-declarations.md)
- [ADR#0066: A Project Is Work Attached Below a Workspace, and Its Initial Access Is Recorded at Creation](./0066-project-resource-and-members-only-access.md)
- [ADR#0068: A Reference Field Says It Is a Reference in Both Its Name and Its Type](./0068-reference-fields-say-they-are-references.md)
- [ADR#0069: Every Event Envelope Records Who Caused It and When Each Step Happened](./0069-event-envelope-records-issuer-and-times.md)
- [ADR#0070: An Agent Declares What It Offers and Whose Authority It Carries; Throughput Ceilings Stay on the Policy Plane](./0070-agent-offered-interface-and-throughput-limits.md)
- [OAuth 2.0 Token Exchange, RFC 8693, the `act` claim](https://www.rfc-editor.org/rfc/rfc8693#section-4.1)
