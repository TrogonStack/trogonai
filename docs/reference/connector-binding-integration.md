# Connector Binding Integration

Connection use follows an explicit, authenticated HTTPS relay. This reference
defines how a provider HTTP request reaches its selected relay without exposing
credentials or proof keys to workload code. It is an integration contract, not
an assertion that an SDK or runtime adapter already implements it.

## Release-approved API destinations

`ConnectorDefinition.egress_hosts` is the outer allowlist for OAuth control,
identity probes and provider API traffic. Membership alone never authorizes
credential injection. `ProxyProfile.api_destinations` separately declares each
eligible API origin as an `ApiDestination`:

| Field | Meaning |
| --- | --- |
| `id` | Stable name within one immutable ConnectorRelease |
| `upstream` | Exact host and port, reached only through verified HTTPS |
| `credential_role` or `anonymous` | One explicit authentication choice |
| `default_for_authentication` | Default only for that role or anonymous mode |

Ids are unique. A host/port and authentication choice identify one entry, and
each authentication choice has at most one default. Every credentialed entry
names a declared access role with a compatible injection rule; refresh tokens,
app private keys and webhook keys are ineligible. Anonymous entries exist only
when the release explicitly supports `Authentication.none`. Their bindings omit
the credential role and never contact custody. Basic credential bundles use
Basic placement; token and API-key roles use Bearer or a reviewed header.

Publish validation requires each API destination and each HTTP restriction host
to lie in the exact reviewed release set. OAuth token, authorization,
registration and revocation endpoints and probe-only hosts receive no API
credential permission merely because they share `egress_hosts`. If a physical
origin serves both purposes, it needs an explicit API destination and role just
like any other origin. This does not grant a workload access to custody-held
OAuth client authentication material.

## Admission and multiple origins

The trusted Session admission caller supplies optional `binding_selections`
alongside the exact AgentRevision declarations. Each `ConnectionBindingSelection`
names one existing declaration, a use mode and, for HTTP only, an optional
`ApiDestinationId`. It carries no URL, secret handle, credential role or signing
key. Selections are authorized deployment configuration and become immutable
plan facts; they cannot be read from a workload request header or environment
variable at call time.

Admission first resolves the declaration to its one permitted Connection. It
then selects the active authentication role. An explicit destination id must
exist in the pinned release for that exact role; otherwise admission refuses.
An omitted id selects only the destination marked as default for that role.
No default fails closed as ambiguous, even if there is a sole nondesignated
entry. An omitted selection means HTTP_PROXY/default; MCP_SESSION and optional
TOKEN_ISSUANCE require explicit selections, and cannot carry an HTTP destination.
Policy may refuse the selected binding; it never chooses a different one.

`ResolvedConnectionUse.proxy_binding` pins the destination id, transport profile,
exact upstream, active credential role, generated binding id and configured
HTTPS relay origin. The resulting grant copies them unchanged. Request-time
admission verifies that the id/upstream/role/profile still describe that same
pinned release, as well as the live Connection generation, permit and Session
egress authority. A bound API origin never switches through redirects or Host
headers.

Multiple legitimate provider origins require separate declaration binding names,
selections and grants, even when they resolve to the same Connection. A single
binding never means an arbitrary member of an origin list. Switching origin
requires using another already admitted binding or a new Session admission;
renewal cannot change routing terms. A new origin on a release requires the
ordinary reviewed release upgrade and authority checks.

## Trusted supervisor adapter

The host exposes an HTTP client bound to the declaration's admitted binding.
This can be a host-side HTTP transport or an authenticated per-attempt IPC
adapter. The contract is the behavior described here; no existing SDK method
name is implied. A workload may construct the provider method, original HTTPS
URL, permitted headers and bounded content. Its URL must have the exact bound
origin, and its path/query must pass the relay's validation. Caller cookies,
Authorization, credential placement fields, relay controls, proof fields and
Accept-Encoding overrides are refused. Forwarded, X-Forwarded-* and Via values
are also owned by the trusted transport; a workload cannot supply them or use
them as a credential placement target.

The supervisor obtains grant and confirmation-key state through the trusted
Session control path. It buffers the request, allocates a fresh invocation id,
constructs the exact relay path and context, adds `Accept-Encoding: identity`
and the content digest, then signs the actual end-to-end request. The adapter
does not expose the confirmation key, a signing oracle for arbitrary bytes,
provider tokens or raw relay controls to the workload. IPC authentication binds
the caller to its execution attempt and fixed allowed binding; a guessed local
socket, port, binding id or grant id grants nothing.

The supervisor presents a safe response against the original provider request
URL and preserves the explicit distinction between a provider status and relay
refusal. The relay prefix is a request-routing detail and is never stripped
from arbitrary provider response content. Provider bodies remain ordinary HTTP
bytes. Protobufs carry configuration and proof metadata, not a duplicated
provider request/response API.

Configuring `HTTP_PROXY` or `HTTPS_PROXY` alone does not implement this contract:
an ordinary HTTPS forward proxy generally receives CONNECT and cannot inject
headers into the encrypted provider exchange. This adapter instead originates
HTTPS to the fixed relay endpoint and lets the relay originate a separate
verified HTTPS connection to the provider. Transparent interception is outside
this profile.

## DNS, connection and credential boundaries

At publish and each new provider connection, parse and normalize the exact
host/port using a trusted URL/host parser. DNS names use lower-case ASCII IDNA
form with no trailing dot; literals require canonical address syntax. Reject
userinfo, wildcard/suffix matching, zone identifiers and non-HTTPS schemes.
Resolve A and AAAA through the trusted resolver, reject the entire result if any
answer is not publicly routable, then connect only to an address in that checked
set. Preserve the original approved hostname for TLS SNI, certificate validation
and HTTP authority. There is no unchecked second DNS lookup in the HTTP client.
Revalidate on reconnect and never accept another authority through connection
coalescing, Alt-Svc, an environment proxy or automatic redirection. These rules
apply to every credential-bearing outbound call, including shared origins.
[OWASP SSRF prevention](https://cheatsheetseries.owasp.org/cheatsheets/Server_Side_Request_Forgery_Prevention_Cheat_Sheet.html)

Network policy additionally denies private, loopback, link-local, metadata,
multicast, special-purpose and reserved destinations for IPv4 and IPv6. A
deployment requiring private provider origins needs an explicitly isolated,
reviewed profile; weakening the public-address check is not a per-request
override. A trusted internal relay may use its configured private origin, but
that service route never grants a workload access to another private upstream.

Disable cookies, automatic redirects, environmental proxy discovery, transparent
decompression and cross-origin connection reuse in the provider client. Pools
are partitioned by approved origin and trusted tenant/custody boundary. Resolved
access values live only in the bounded injection path and are discarded after
use. No provider body, request path/query, header value, signature, key or access
value enters a persisted routing record, log, trace or error message.

## Reviewed response and authentication policy

An optional `ProxyProfile.response_policy` names a `ResponsePolicyId` and the
digest of the exact reviewed declarative policy JSON bytes. It is an immutable
release pin, not an executable artifact name or evidence that a deployed filter
exists. The policy fixes provider-specific authentication/scope classification
and any stronger response handling. Classification uses only the policy's
bounded typed predicates and reviewed fields, never arbitrary status/body text
or a language-model judgment.

Admission and runtime both require the exact policy id, matching bytes and
supported policy version. Unknown id, absent runtime support, invalid policy,
digest mismatch or an unavailable required filter fails closed; none silently
falls back to generic handling. Validate the original bytes before parsing,
reject duplicate JSON members, and retain the digest as the policy version.
No request or response body is added to an audit record to explain a match.

With no stronger policy, the documented generic transport and detectable-
credential protections still apply. A bare provider 401/403 or a GraphQL 200
does not by itself establish which credential, scope or provider-side action
failed. A profile requiring a trusted filter cannot use its absence as a weaker
default. Provider fixtures must identify their selected policy and the runtime
acceptance tests needed before enabling it.

## Conformance boundary

Schema checks prove that destination identities, roles, profile values and
selection relationships have the required shape. Runtime integration must prove
origin matching, authenticated IPC, key isolation, exact signed bytes, mixed A/
AAAA rejection, pinned dialing, TLS name preservation, no redirect/coalescing
escape, live revocation and bounded content handling. [HTTP transport](./connector-http-transport.md)
and [the relay reference](./connection-http-proxy.md) define the companion rules.
