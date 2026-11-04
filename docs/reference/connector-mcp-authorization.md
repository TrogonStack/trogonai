# Connector MCP Authorization Profile

The connector authorization profile explicitly adopts
[MCP authorization, revision 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization).
This is an OAuth-client authorization pin. It does not upgrade MCP transport,
session, tool or extension protocols, and it does not claim whole-protocol
conformance. A release and its discovery snapshots record this authorization
revision through `McpSpecificationRevision`.

## Discovery and authorization-server binding

Use the discovery order and network protections in
[ADR#0094](../adr/0094-oauth-authorization-protocol.md). Validate protected
resource metadata and authorization-server issuer before using any endpoint.
Keep each preregistered or dynamically registered app's credentials bound to
its exact issuer and pinned snapshot. An updated resource document selecting a
different authorization server cannot reuse another issuer's client secret,
registration token or access token. A Client ID Metadata Document URL may be
portable, but this platform still records a separate issuer-pinned ProviderApp.
The new issuer requires its own validated snapshot and consent boundary.

These requirements match the current
[discovery](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/authorization-server-discovery)
and [registration](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization/client-registration)
profiles. Existing PKCE S256, HTTPS endpoints, exact redirect registration,
resource/audience binding and no-token-passthrough rules remain in force.

## Scope selection and step-up

The current authorization flow accumulates the previously requested scopes
with the current challenge. Keeping only the scopes that were granted can lose
an earlier request when optional scopes were denied. Our dispatch algorithm is:

```text
previous_requested = union(prior_attempt.required_scopes,
                           prior_attempt.optional_scopes)
candidate = union(previous_requested, current_challenged_scopes)
```

Resolve the prior attempt through the current `AuthorizationRecord.attempt_id`
and verified custody parent. This authoritative history is required; absence
is a typed unavailable/integrity outcome, never permission to substitute the
current grant or assume an empty previous request. Use exact scope tokens for
MCP, remove duplicates and sort them deterministically for the wire request.

Before creating the new attempt, check the candidate against the current
release, app ceiling, live owner authority and product policy. If any requested
scope is outside the permitted set, refuse the step-up rather than silently
intersecting away a previous request. The user sees the complete scope request
and gives new consent. Previously requested scopes are not evidence of a grant:
the new token response still determines actual granted authority, and existing
scope-reduction and authority-widening protections apply. This flow changes no
other Connection identity boundary.

The initial flow continues to prefer a challenge over resource metadata scope
defaults, without assuming a subset relationship. Challenge-derived requirements
still pass local ceilings and the refresh-specific rules below. Never replay a
provider mutation merely because a scope upgrade completed. The invocation
recovery contract determines whether the original operation may be retried.

## Refresh intent and offline access

Generated CIMD and dynamic-registration metadata must describe actual client
behavior. Use `grant_types: ["authorization_code"]`, adding `"refresh_token"`
when the configured connection needs refresh capability, and
`response_types: ["code"]`. This expresses intent; it does not prove the server
will issue a refresh token. Missing refresh material follows the pinned release's
explicit optional-refresh policy and access expiry, with reconnect when needed.

`offline_access` is an authorization-server permission, not a generic resource
scope. Request it only when background refresh is needed, the validated
authorization-server `scopes_supported` explicitly contains it, and current
ceilings and consent allow it. A resource metadata document or challenge alone
cannot cause that scope to be added. If a server incorrectly lists it there,
derive any request solely from the independently validated authorization-server
capability and the product's refresh intent. Do not infer support from the name
of a provider or a token returned on another connection.

For OIDC offline access, the broker requests `prompt=consent`; this platform
does not use the specification's alternative prior-consent conditions. The
parameter is dispatch-owned and cannot be supplied by agent code or a browser.
The token request remains bound to the exact resource and original consent.
Refresh credentials stay inside custody. These local dispatch rules implement
the current MCP refresh guidance and the consent requirements in
[OpenID Connect Core, Offline Access](https://openid.net/specs/openid-connect-core-1_0.html#OfflineAccess).

## Issuer validation before success or error handling

`AuthorizationRequest.expected_issuer` comes from the validated selected
metadata before redirect and is pinned beside state and PKCE. The advertised
`authorization_response_iss_parameter_supported` flag is pinned as
`response_issuer_required`. The callback validates the platform session,
initiator binding and single-use state before processing the provider result.

Parse the bounded callback form/query once. Reject duplicate singular
parameters and malformed UTF-8. Decode `iss` using the form encoding exactly
once, then compare it to the stored issuer as a whole string. Do not apply URI
case folding, default-port removal, trailing-slash changes or percent-decoding
again. For example, an encoded literal `+` must remain a literal `+` after the
form decode; it is not a space or a second URI-normalization opportunity.

| Pinned advertisement | Callback issuer | Decision |
| --- | --- | --- |
| Required | Missing | Refuse |
| Required | Present | Require exact decoded equality |
| Not required | Present | Require exact decoded equality |
| Not required | Missing | Continue only after all other callback bindings pass |

Apply this order to error callbacks as well. A mismatched or missing-required
issuer cannot cause an exchange, a provider-error event, UI display of
`error_description`/`error_uri`, a link navigation or an error-specific retry.
After issuer validation succeeds, claim the callback and map only recognized
provider error codes to the closed failure enum. Provider text remains outside
events, logs and public errors. See
[RFC 9207](https://www.rfc-editor.org/rfc/rfc9207.html#section-2.4) for issuer
validation; the unconditional check of a present issuer also follows the
adopted MCP profile's local-policy rule.

## Dynamic registration fallback

Prefer an existing issuer-specific preregistered app, then CIMD. Dynamic Client
Registration is deprecated in the adopted MCP profile and remains a supported
fallback only when those choices are unavailable and a validated registration
endpoint is present. The broker keeps the existing write-ahead and ambiguous
outcome rules; adjusting metadata never authorizes a blind repeat of an
uncertain registration POST.

The server-hosted browser product sends `application_type: "web"` explicitly
in a dynamic registration request. Its redirect URI is the environment's exact
HTTPS callback, not a caller-selected localhost address. A native or local
development client uses a separately configured app with appropriate native
metadata; it cannot mutate the production web app or reuse its client material.
Unsupported redirect constraints produce a typed registration failure with no
provider body. The client metadata behavior follows
[OpenID Connect Dynamic Client Registration](https://openid.net/specs/openid-connect-registration-1_0.html#ClientMetadata).

The existing app kinds already carry issuer, snapshot and callback bindings.
These fixed dispatch rules do not require a new browser request parameter or
an arbitrary client-metadata map.

## Resource URI preservation

Resource identity and issuer identity use separate rules. Resource construction
uses the supported MCP URI profile with a lowercase scheme/host while preserving
the significant path bytes. Prefer an omitted trailing slash only when it is
known not to change server identity. A significant `/mcp/` remains `/mcp/` in the
resource, discovery binding, token request and account fingerprint boundary.
It must not become `/mcp` during authorization or dispatch.

This platform profile rejects resource queries and fragments as unsupported;
it does not strip them and claim to support the resulting URI. The typed
`CanonicalResourceUri` now permits trailing slashes. Existing connections are
not rewritten by this schema amendment: a previously selected different
resource requires a new identity/consent boundary. The current
[canonical-server-URI guidance](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization#canonical-server-uri)
allows a significant trailing slash.

## Account identity and verification boundary

Authorization discovery and token audience checks do not identify the provider
account needed for this platform's claim and reconnect checks. Require the
reviewed identity strategy in
[Connector Account Identity](./connector-account-identity.md), or return typed
`PROBE_UNSUPPORTED` before account claim or activation. Do not infer a stable
provider identity from arbitrary MCP tool output or an unverified token payload.

Conformance cases must cover a prior requested scope that was not granted,
ceiling changes, conditional offline access, absent refresh material, every
issuer-presence combination on both success/error callbacks, encoded issuer
values, cross-issuer credential reuse, web/native registration separation and
significant trailing-slash round trips. A compiled-schema regression covers
URI shape only. Runtime callback ordering, HTTP requests, custody behavior and
provider interoperability require their own acceptance tests.

[Synthetic authorization conformance cases](/connector-fixtures/connector-mcp-authorization-v1.json)
record positive and negative inputs with expected scope, issuer, refresh,
registration and revision-pin decisions. They are normative cases for future
real client tests. Parsing the fixture or checking its internal expectations
does not prove an OAuth flow, callback ordering or runtime enforcement.
