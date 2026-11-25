# Connection HTTP Proxy

This is the v1 contract for ordinary provider HTTP forwarding under draft
[ADR#0096](../adr/0096-connection-use-by-agents.md). It defines the wire boundary;
it does not claim an implemented or deployed proxy. Request and response bodies
stay HTTP. Protobufs carry configuration, authority, transient control metadata
and safe audit facts.

## Binding and authorization

`ProxyBinding` fixes a nonsecret binding id, the configured HTTPS relay origin,
one exact provider host/port and a credential role. Admission chooses the
binding from trusted configuration and the pinned release; the grant copies it
unchanged. A request cannot select an alternate host, role or SecretRef. Only
anonymous connections omit the role, and they never resolve a credential.

A live `HTTP_PROXY` grant binds Session, execution attempt, AgentRevision, plan
digest, connection/configuration, authorization generation, confirmation key,
audience and expiry. Its audience is `HTTP_PROXY`; `MCP_SESSION` and optional
`TOKEN_ISSUANCE` have separate audiences. The old `INVOKE_OPERATION` enum value
is reserved. A binding id or grant id by itself grants nothing.

The relay checks live standing, owner/delegation, connection permission and
Session egress policy for the logical upstream before custody. The relay origin
is internal routing, not the destination that authorizes arbitrary provider
access. Custody independently checks the authenticated proxy role, exact grant
and binding, tenant, live authority and audit acceptance, then derives the
SecretRef from the connection. It never trusts a requester-selected SecretRef.

Grant and connection references carry typed `resource_parent` values. The
Session grant project may differ from Connection custody; authorization resolves
both independently. See [Connector Resource Parents](./connector-resource-parents.md).

## HTTP route and request proof

The supervisor routes to the grant's HTTPS relay origin. The origin-form path
is `/v1/connection-proxy` followed by the original provider path; the original
query is preserved. The relay strips that prefix exactly once and forwards
only to the binding's fixed upstream over verified HTTPS. It is an explicit
relay with its own TLS endpoint, not a CONNECT tunnel or TLS interception.

The supervisor outside agent code supplies:

| HTTP field | Meaning |
| --- | --- |
| `Trogon-Connection-Context` | Base64url encoding of exact `ProxyRequestContext` protobuf bytes: `ProxyBindingRef` and invocation id |
| `Content-Digest` | RFC 9530 `sha-256` digest of the actual request content, including empty content |
| `Signature-Input` and `Signature` | RFC 9421 signature with label `connection`, algorithm `ed25519`, and key id equal to the unpadded base64url RFC7638 SHA-256 public OKP JWK thumbprint (RFC8037) |

The signature covers `@method`, `@target-uri`, content digest, connection
context and every forwarded end-to-end header. The proxy obtains the attested
confirmation key from service state, never a caller key URL. Created, expires
and nonce parameters are required; the nonce equals the invocation id. Proof
validity is at most 60 seconds and cannot exceed grant expiry. A future-created
or expired proof fails. Unsigned forwarded headers are refused.

The decoded context is limited to 4 KiB. Unknown fields, duplicate singular
fields and malformed bindings are refused. Signatures authenticate the exact
received bytes; protobuf is not assigned a universal canonical encoding. Valid
semantically equivalent encodings have different signatures and remain subject
to the same strict descriptor/wire preflight. Request content
is buffered within its size bound and its digest verified before custody or
forwarding. Digest trailers are unsupported. Conflicting framing is refused;
the relay rebuilds authority, framing and hop-by-hop fields rather than blindly
forwarding them. Control and proof headers never reach the provider.

Nonce consumption is atomic across replicas for `(grant, invocation id)`, with
a replay tombstone through final grant expiry or revocation. Renewal extends
existing tombstones and never resets consumption; lost consumption state fails
closed. Duplicate signed requests are refused;
a missing or unknown provider response is never permission to repeat an effect.
An attributable authentication rejection may trigger health/refresh work for
a later request. V1 does not automatically replay the accepted request after
refresh. A fresh signed HTTP request cannot bypass the invocation tombstone or
be minted automatically to hide an uncertain effect. The
[recovery contract](./connector-relay-recovery.md) defines the authoritative
receipt journal, durable completion outbox, crash matrix and restore fencing.

The selected [finite transport profile](./connector-http-transport.md) and
[API binding rules](./connector-binding-integration.md) also apply. Streamed,
compressed, ranged and upgraded representations are unsupported in v1.

## Enforceable restrictions

Allowed methods are GET, HEAD, POST, PUT, PATCH, DELETE and OPTIONS. CONNECT,
TRACE and extension methods are unsupported. A request rule matches exact
host/port, an allowed method and a literal path prefix at a segment boundary:
`/issues` matches `/issues` and `/issues/123`, not `/issues-old`.

Rules within a set are alternatives. Every release, planned-grant and live
permit set must admit the request. An omitted set adds no restriction; a
present empty set is invalid. Unsafe paths are refused before matching:
backslashes, invalid percent escapes, encoded separators, encoded or literal
dot segments, repeated slashes, userinfo, fragments and absolute-form targets.
The authorized path must be exactly the forwarded path after removing the
fixed relay prefix; later normalization cannot change its meaning.

Credential-scope ceilings are allowlists of actual credential scopes. A token
with broader scopes is denied, not magically attenuated. HTTP method/path
restrictions do not establish field-level or GraphQL read-only authorization.
Unsupported restrictions and mode-inapplicable ceilings fail before custody.
MCP tool listing/call restrictions belong to the trusted MCP executor.

## Credential and response handling

`ProxyProfile` declares role-based Bearer, Basic or reviewed custom-header
placement. Values come from custody. Caller Authorization, Cookie, proxy
credentials or injection-target headers are rejected. Header injection cannot
alter routing, framing or relay controls. Query, cookie and body credential
placement are unsupported. The proxy has no OpenBao client, refresh token,
provider-app secret or app private key.

Responses retain provider HTTP status, permitted headers and bounded body.
Redirects are not followed. Set-Cookie and credential-bearing Location fields
are removed, as are all upstream relay-control fields. The bounded response is
buffered before release; a detected credential echo fails closed. Arbitrarily
encoded or transformed credential echoes are not claimed safe by a generic
schema. Providers needing stronger response handling require a reviewed trusted
filter, separate from mandatory API output schemas.

## Response provenance and failures

The relay owns `Trogon-Connection-Response`: `upstream` marks a provider response;
`proxy` marks a relay refusal. Upstream `Trogon-Connection-*` fields are stripped
before this marker is written. Relay errors have an empty body,
`Cache-Control: no-store`, and a base64url `ConnectionProxyFailure` in
`Trogon-Connection-Failure`. Provider HTTP errors keep their own status and body
within the response policy; internal classification does not replace them with
a protobuf response envelope.

| Relay result | HTTP status |
| --- | --- |
| Grant/proof mismatch | 401 |
| Permission refusal, blocked release or unsupported restriction | 403 |
| Unavailable/superseded connection or replayed request | 409 |
| Invalid request | 400 |
| Platform/provider budget blocks dispatch | 429 |
| Provider unavailable, oversized response or credential response rejected | 502 |
| Provider outcome unknown | 504 |
| Audit, projection or internal failure | 503 |

Throttle source and permitted retry delay stay typed. A retry delay never
proves replay is safe. Provider 401/403 observations affect connection health
only when the provider profile can attribute their authentication meaning.

## Lifecycle and audit boundary

Acceptance is durable before credential resolution or provider dispatch.
Completion records safe status, sizes, timing and authority correlation. Bodies,
raw paths/query, arbitrary headers, proofs, content digests and provider error
text do not enter audit streams. Nonces and grant/request correlation identify
a use without storing its content. Revocation, stale policy, tenant mismatch or
an unavailable authorization read fails closed.

The callback stores a versioned keyed initiator binding, not a principal copied
into a command payload. Trusted dispatch derives it from verified session
identity for the exact tenant/attempt and compares it with the recorded binding,
in addition to the browser cookie and signed-state checks. See
[OAuth Authorization Protocol](../adr/0094-oauth-authorization-protocol.md).

## Related contracts

- [Third-Party Connections](../architecture/third-party-connections.md)
- [Credential custody and refresh](../adr/0095-connection-credential-custody-and-refresh.md)
- [RFC 9421: HTTP Message Signatures](https://www.rfc-editor.org/rfc/rfc9421.html)
- [RFC 9530: Digest Fields](https://www.rfc-editor.org/rfc/rfc9530.html)


The [conformance artifacts](./connector-conformance.md) specify exact HTTP
signature bytes and key-thumbprint construction, with positive/negative examples.
Runtime acceptance must use those artifacts against the deployed parser, ledger,
network and custody adapters independently.
