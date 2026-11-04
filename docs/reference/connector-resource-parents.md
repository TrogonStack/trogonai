# Connector Resource Parents

Connector contracts distinguish resource containment, hierarchy placement and
logical ownership. The schemas in `connections/types` and `connections/resources`
make containment explicit. They do not implement a gateway or enable generated
clients by themselves.

## Placement, containment and owner

| Concept | Representation | Responsibility |
| --- | --- | --- |
| Hierarchy placement | `NodeId parent` on resources that attach directly to the hierarchy | The hierarchy owns current placement and moves |
| Resource containment | Typed `resource_parent` on connector commands, events, state, references and names | Fixes the storage and authorization namespace |
| Public collection parent | Typed `parent` on create/list request envelopes | Identifies the collection the request addresses |
| Logical connection owner | `ConnectionOwner` | Decides administration, delegation and offboarding |

[ADR#0040](../adr/0040-contract-field-vocabulary.md) reserves bare domain
`parent` for hierarchy placement. [ADR#0066](../adr/0066-project-resource-and-members-only-access.md)
makes a Project a resource, not a NodeId. A connector's project container is
therefore `ProjectResourceParent`, not a fabricated hierarchy node. Public
collection-parent parameters follow the separate management naming rule in
[ADR#0046](../adr/0046-project-anchored-resource-hierarchy.md).

Moving a project in the hierarchy changes neither its child names nor their
storage keys. A person or workspace logical owner does not move a Connection
out of its custody project. A credential's subject is an association, not an
alternative custody parent.

## Resource containers and names

Resource-name messages are typed identities. They contain `resource_parent`
and an opaque resource id, not mutable labels or secret material. Get requests
use these identity-only values; callers need no configuration digest merely
to identify a resource. Pinned references still add the configuration or release
digest needed for execution.

| Resource | Parent type | Canonical public name |
| --- | --- | --- |
| ConnectorRelease | `ConnectorReleaseParent` containing ConnectorId | `platform/connectors/{connector}/releases/{release}` |
| ProviderApp | `ProviderAppParent`, project or platform | `projects/{project}/provider-apps/{app}` or `platform/provider-apps/{app}` |
| Connection | Custody `ProjectResourceParent` | `projects/{project}/connections/{connection}` |
| AuthorizationAttempt | Custody `ProjectResourceParent` | `projects/{project}/authorization-attempts/{attempt}` |
| ProviderAccountClaim | Custody `ProjectResourceParent` | `projects/{project}/provider-account-claims/{claim}` |
| ConnectionGrant | Session's `ProjectResourceParent` | `projects/{project}/connection-grants/{grant}` |
| Credential | `CredentialResourceParent`, project or platform | `projects/{project}/credentials/{credential}` or `platform/credentials/{credential}` |
| ConnectionPermission | `ConnectionPermissionParent` containing PolicySubject | `agents/{agent}/connection-permissions/default` or `nodes/{node}/connection-permissions/default` |

The platform marker is an explicit typed namespace, with no tenant hierarchy
node. A customer cannot publish into it by supplying the marker. Releases are
under one platform connector namespace. Customer apps and their registration
credentials are project-scoped; platform app material is platform-scoped.

Attempts and claims remain flat children of the custody project. Their target
Connection is an association, not another public path segment. A released claim
can serve a later holder without changing its parent. Grants are flat children
of the Session project, with the Session recorded separately. Their parent may
differ from the connection's custody parent, so those values must not be forced
equal.

The erasable provider-account profile includes custody parent and connection
identity. Its key includes tenant, custody project and ConnectionId. Profile
personal data stays out of streams, public resource wrappers and audit facts.

## Canonical path formatting

Typed ids are authoritative. Format each id as one UTF-8 path segment using
RFC 3986 percent encoding: leave unreserved ASCII bytes literal, percent-encode
other bytes with uppercase hex, and never normalize Unicode or decode twice.
A slash inside an opaque id is encoded, never interpreted as a hierarchy edge.
The canonical name has no leading slash, trailing slash, query or fragment.
HTTP endpoints may add their version prefix outside this resource name.

A textual gateway parser must round-trip exactly to that canonical form and
reject alternate spellings or an unknown collection before dispatch. It must
not let a decoded delimiter, a display label or a mutable hierarchy path select
a different resource. These management-path rules are separate from the relay's
stricter provider request-path rules.

## Creation, state and routing invariants

Creation commands carry `resource_parent`; genesis events record it; the present
folded state retains it unchanged. Later commands and events targeting a bare
aggregate id carry that same parent. The fold rejects a parent different from
its recorded parent. Transport routing must also agree, so an id alone cannot
select a same-id stream in another project or namespace.

A create envelope's `parent` must equal the nested command's `resource_parent`.
List envelopes require a typed parent. Get envelopes require a complete typed
resource name. Returned wrappers require their resource name, folded-state id
and folded-state parent to agree. Parent agreement is a schema constraint;
permission to use that parent is a separate authenticated admission check.

Cross-resource rules are explicit:

- A project-owned Connection's owner project equals its custody parent. Other
  owner kinds retain their separate ownership and delegation rules.
- An authorization attempt, its claim and its staged material remain in the
  target Connection's custody project.
- A project ProviderApp's material uses the same project parent; platform app
  material uses the platform parent. A connection or staged attempt cannot
  select platform custody.
- A Connection and its attempt may select an app in their custody project or
  the platform namespace. Node-scoped apps and borrowing another project's app
  require a later explicit sharing contract.
- A ConnectionPermission's parent subject equals its aggregate-key subject.
- A grant's project is checked against the authoritative Session project,
  rather than inferred from the granted Connection.
- A pinned ConnectorReleaseRef or ProviderAppRef includes its own container;
  it is never resolved globally by a bare id.

These are invariants, not new cross-aggregate reads inside pure folds. A fold
checks the parent and facts carried by its own command/event/state. Trusted
dispatch verifies external Session, owner, app, attempt and hierarchy facts
before issuing a command.

## Authorization and pagination

The gateway authenticates the caller, derives tenant and project authority from
verified context and live policy, authorizes the target collection, and compares
the route with the typed payload before dispatch. A caller-supplied parent is
not an authorization grant. Unknown and unauthorized resources must not become
a cross-project existence oracle.

Platform catalog/app operations are restricted to platform operators. Claim,
grant, credential-record and permission genesis envelopes do not grant ordinary
users the ability to mint service-owned state. Each command retains its existing
caller class and lifecycle preconditions. Material submission uses the separate
sealed custody contract; resource management never returns raw credentials.

List pagination is bounded and parent-scoped. A page token grants no authority:
validate it against the requested parent and query parameters and reauthorize
every page. Do not interpret a token as an instruction to switch namespace or
continue a list after permission was removed.

Paged collections default to 50 entries and accept at most 200, ordered by
bytewise opaque id. Cursors bind caller, tenant, parent, resource type, ordering
and read snapshot. ConnectionPermission is an unpaginated singleton with at most
one result per exact policy subject.

The create/list/get envelopes specify typed resource access only. Browser
start-connect, consent redirect, reconnect and disconnect orchestration still
need their product-facing workflow contract; a parent-bound domain command is
not a browser onboarding API.

## Compatibility and implementation boundary

Old container fields are retired with reserved tags and names where replaced.
The new required typed fields must be supplied explicitly; an old parentless
message is not silently upgraded or assigned the caller's default project.
SecretRef remains an opaque custody handle. Resolve, refresh and signing
operations derive its authoritative parent from verified service state where
they intentionally use opaque references instead of public resource names.

Schema checks validate message shape and parent equality. They do not establish
runtime authorization, stream isolation, gateway canonicalization, pagination
or replay enforcement. Code generation and service implementation remain
separate work.

## Related documentation

- [Third-Party Connections](../architecture/third-party-connections.md)
- [Third-party connection model](../adr/0092-third-party-connection-model.md)
- [Connection ownership and delegation](../adr/0093-connection-ownership-and-delegation.md)
- [Connection HTTP Proxy](./connection-http-proxy.md)
