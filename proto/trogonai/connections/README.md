# Third-party connection contracts

Provider API requests and responses stay HTTP. The authorized connection proxy
checks a live grant, resolves credentials from custody, injects them and forwards
the request. Agent code receives no provider credential. Custody owns OAuth code
exchange, refresh, token persistence and version settlement.

[ADR#0096](../../../docs/adr/0096-connection-use-by-agents.md) records the decision.
[Third-Party Connections](../../../docs/architecture/third-party-connections.md)
explains it, and [Connection HTTP Proxy](../../../docs/reference/connection-http-proxy.md)
specifies the explicit HTTPS relay, signed request fields and failure profile.

## Contract map

| Surface | Responsibility |
| --- | --- |
| `types.ProxyBinding` and `ProxyBindingRef` | Immutable relay origin, exact upstream and credential-role binding to a live grant |
| `access.ProxyRequestContext` | Ephemeral signed HTTP control metadata, with no provider body or credential |
| `access.ConnectionProxyFailure` | Relay-owned HTTP failure metadata, distinct from preserved provider responses |
| `catalog.ProxyProfile` | Credential placement, enforceable restrictions, resource bounds and quota feedback |
| Admission, grants and permissions | HTTP_PROXY/MCP_SESSION/optional TOKEN_ISSUANCE use, matching audiences and live policy |
| Connection audit | Safe correlation and outcomes without provider bodies, raw paths, request digests or proofs |
| `catalog.OAuthProfile` | Reviewed stage-specific public parameters, strict token-response interpretation and expiry policy |
| `types.AccessExpiry` | Reported, documented, unknown or explicitly documented non-expiring evidence |
| `secrets/exchange.ResolveForTrustedUse` | Credential selection derived from verified proxy, MCP or staged-attempt authority |
| `secrets/exchange.EnsureProviderAccess` | Separate optional future trusted-adapter issuance with a direct sealed reply |

The former InvokeOperation RPC and mandatory operation catalog are removed.
Retired field numbers/names and enum values are reserved; they are not reused
with new meanings. Lifecycle streams for connections, provider apps, consent,
claims, credentials, grants and permits remain separate aggregates.

## Resource parents

Typed `resource_parent` is explicit on routed commands/events, present state,
references, resource names and read models. Public create/list envelopes carry
`parent`, while get requests use identity-only resource names. Returned resources
validate parent and identity equality against folded state. The
[resource-parent reference](../../../docs/reference/connector-resource-parents.md)
defines project/platform/subject containers and their public names. None of
these containers is a fabricated hierarchy NodeId or a logical owner.

## Shared identities and values

Callback initiator matching uses `SessionInitiatorBinding`, computed only by
trusted dispatch from verified identity for the pinned tenant and attempt.
Raw initiator/session principal fields are retired and reserved. Event actors
remain in the envelope.

The erasable provider account profile is keyed by tenant, custody project and ConnectionId, as
[ADR#0093](../../../docs/adr/0093-connection-ownership-and-delegation.md) specifies; its fingerprint joins claims and stream facts. Credential
aggregates are keyed by opaque SecretRef; a staged bundle and its promoted live
bundle are separate streams. AuditCorrelationId is a shared correlation value.
The exchange RPCs reuse custody value messages without importing foreign
aggregate commands, events or folded state into another aggregate.

## Schema and runtime boundaries

These schemas define contracts only. Buf validation of schema examples does not
enable production validation or prove a deployed relay, nonce store, policy
source, audit sink or secrets service. Code generation and runtime implementation
are outside this change. Deployment isolation and provider-specific onboarding
remain separate work.


## Security and workflow profiles

The current contracts select finite buffered HTTPS, explicit API destinations
and response-policy pins, a dedicated custody identity operation with fenced
lookup-key migration, durable relay recovery and a typed product control facade.
See the [security profile](../../../docs/reference/connector-security-profile.md),
[conformance artifacts](../../../docs/reference/connector-conformance.md), and
[MCP authorization profile](../../../docs/reference/connector-mcp-authorization.md).
Code generation and deployed boundary/provider acceptance remain separate work.
