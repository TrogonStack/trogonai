# Third-Party Connections

A connection lets an agent call a provider through an authorized connection HTTP proxy
while credential material stays under platform custody. The agent constructs
a normal HTTP request; the proxy checks authorization, obtains an access
credential, injects it and forwards the request. The provider's response comes
back as HTTP.

This page explains the target architecture in draft
[ADR#0096](../adr/0096-connection-use-by-agents.md). The HTTP proxy call path
supersedes the earlier mandatory typed-operation design. The protobuf drafts now define that boundary, including the explicit HTTPS
relay and grant binding. This documentation does not establish an implemented
or verified proxy.

Resource containers and public create/list/get parents are defined in
[Connector Resource Parents](../reference/connector-resource-parents.md).
`resource_parent` identifies custody or namespace; logical owner, associated
Session and hierarchy placement remain separate.

## HTTP carries provider traffic

```text
Agent HTTP request -> connection HTTP proxy -> provider HTTP endpoint
                            |
                            v
                    secrets service
                 resolve / refresh / revoke
```

The return path carries the provider's HTTP status, permitted headers and body.
The proxy applies authorization, destination restrictions, resource limits and
credential protections. It does not require a platform protobuf request and
response schema for every provider endpoint, nor convert arbitrary provider
responses into a typed operation result.

The normal path does not hand a provider token to agent code. The trusted proxy
can hold access credentials briefly in memory; the secrets service holds the
refresh credentials and performs OAuth token exchanges and refreshes. Provider
HTTP parsing stays outside the process that owns OpenBao access.

## Protobuf defines authority and lifecycle

| Concern | Contract responsibility |
| --- | --- |
| ConnectorRelease and ProviderApp | Pin provider trust, approved hosts, authentication metadata and app identity |
| Connection and account claim | Record consented authority, owner, configuration and account continuity |
| AuthorizationAttempt | Track consent, callback claim, code exchange and its known or unknown outcome |
| Credential custody | Track references, version eligibility, refresh transactions, revocation and cleanup |
| Connection permissions and grants | Authorize a specific connection for a Session and execution attempt |
| Proxy binding | Bind authenticated use to the connection/grant, allowed destination and credential placement; fixed by `ProxyBinding` in the grant |
| Audit and health facts | Record safe authority/outcome metadata and lifecycle changes without request bodies or secrets |

The request method, path, permitted headers and body belong to HTTP forwarding.
Provider-specific SDKs or higher-level tools may use that path, but an exhaustive
operation catalog is not a prerequisite for a connector.

The provider-specific metadata that remains necessary concerns authentication:
OAuth authorization parameters, token response interpretation, refresh
behavior, credential placement and the limited account identity probe. Token
endpoint response parsing is distinct from mapping every ordinary provider API
response.

## The proxy checks authority before resolving a credential

Admission resolves a declared connection before an attempt receives a grant.
Each request must bind to that admitted connection and pass live permission,
standing, authorization-generation and destination checks. A grant identifier
alone is not authority. Child Sessions receive their own grants, and removing
a permit or revoking a connection stops future use within the configured
revocation bound.

Budget checks and durable audit acceptance precede credential resolution and
provider dispatch. Request and response bodies, tokens and credential-bearing
URLs are excluded from retained audit facts. Shared app quotas and refresh
budgets prevent one tenant from consuming all provider capacity.

Generic HTTP forwarding does not prove field-level or read-only access to an
API body. Any finer policy needs an explicitly defined enforcement mechanism.
MCP tool allowlists similarly remain the responsibility of a protocol-aware
trusted MCP caller, not an assumed property of a generic HTTP proxy.

## Ordinary HTTP has a concrete relay contract

[Connection HTTP Proxy](../reference/connection-http-proxy.md) defines the
explicit authenticated HTTPS relay. The supervisor routes a signed request to
the configured relay origin; the grant fixes the provider host and credential
role. The relay checks both logical upstream egress permission and connection
authority, and custody independently derives the matching credential.

The supervisor uses HTTP Message Signatures over method, target, control context,
content digest and forwarded headers. Nonces are consumed once across replicas.
The response carries a relay-owned provenance marker, preserving provider HTTP
responses while distinguishing the proxy's own typed failures. This mechanism
terminates its own HTTPS connection and opens a separate provider connection;
it needs no workload TLS interception.

The former `InvokeOperation`, mandatory endpoint schemas and operation refs are
retired. Their field numbers and enum values are reserved. HTTP proxy and MCP
session modes are distinct, and unsupported or mode-inapplicable restrictions
are refused. Optional `secrets/exchange.EnsureProviderAccess` has a direct sealed
reply for registered trusted adapters and is separate from ordinary proxy use.

OAuth metadata handles fixed public request parameters and exact token-response
field selectors. Access expiry records provider-reported, documented, unknown
or documented non-expiring evidence. Local credential validity is always bounded
separately; an absent provider lifetime is never replaced with a made-up expiry.

## Related decisions

- [Third-party connection model](../adr/0092-third-party-connection-model.md)
- [Ownership and delegation](../adr/0093-connection-ownership-and-delegation.md)
- [OAuth consent protocol](../adr/0094-oauth-authorization-protocol.md)
- [Credential custody and refresh](../adr/0095-connection-credential-custody-and-refresh.md)
- [Agent use through an HTTP proxy](../adr/0096-connection-use-by-agents.md)
- [Secret management](./secret-management.md)
- [Provider credential custody](./provider-credential-custody.md)
- [Workload identity](../reference/workload-identity.md)


## Security and integration references

The [security profile](../reference/connector-security-profile.md) selects
operational defaults rather than leaving every deployment choice open.
[Account identity](../reference/connector-account-identity.md),
[product control](../reference/connector-control-api.md),
[HTTP transport](../reference/connector-http-transport.md),
[binding integration](../reference/connector-binding-integration.md) and
[receipt recovery](../reference/connector-relay-recovery.md) define the detailed
boundaries. The [Linear candidate](../reference/connector-linear-profile.md)
records current provider evidence and explicit restrictions. The
[conformance gate](../reference/connector-conformance.md) separates local
contract/vector checks from runtime/provider acceptance.
