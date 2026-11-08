# Linear Connector Admission Profile

This is the reviewed candidate for the initial project-owned Linear connector.
It is a contract/profile fixture, not an app registration, enabled release or
live provider acceptance. Source review date: 2026-11-03.

The [definition fixture](/connector-fixtures/linear-connector-definition-v1.json) contains
no client id, secret, callback deployment hostname or token. Custody/ProviderApp
configuration supplies those separately. Publishing creates new immutable
definition bytes/digest; it does not patch an existing release.

## Selected provider boundary

| Item | Profile choice |
| --- | --- |
| Account actor | `actor=app`, subject kind APP, to attribute automation to the app |
| Initial authority | Required `read`; no write/admin scope by default |
| Code flow | S256 PKCE plus confidential client authentication in the broker |
| Authorization | `https://linear.app/oauth/authorize` |
| Exchange/refresh | `https://api.linear.app/oauth/token` |
| Revocation | `https://api.linear.app/oauth/revoke`, standard `token` form field |
| Ordinary API | Explicit destination `api`, HTTPS `api.linear.app:443`, POST under `/graphql` |
| API credential | Role `access`, Bearer access credential only |
| Issuer boundary | Release-configured `https://linear.app`; no claim of OIDC metadata support |

Linear documents [app actor authorization](https://linear.app/developers/oauth-actor-authorization),
[OAuth/PKCE and token lifecycle](https://linear.app/developers/oauth-2-0-authentication)
and its [GraphQL endpoint](https://linear.app/developers/graphql). Our least-scope
and role/destination restrictions are platform policy. A later write-capable
profile requires explicit reviewed scope and new consent; an HTTP POST rule
alone does not establish GraphQL read-only behavior.

Authorization scopes are comma-delimited, while this profile selects explicit
space-delimited response scopes. Legacy pre-December-2023 apps returning a scope
array need their own immutable response profile; the broker never tries both
formats. Access expiry uses reported integral `expires_in`, not a hard-coded
provider lifetime. Missing/malformed expiry fails this curated profile.

Use rotating refresh without reuse grace in this initial profile. Linear's
documented grace is not enabled automatically: relying on it requires a separate
recovery proof. A missing replacement refresh credential cannot authorize reuse
of the old one. Revoke errors remain explicit provider outcomes; they are not
reported as successful remote erasure.

## Minimal identity probe

```graphql
query ConnectorIdentity { viewer { id organization { id } } }
```

The public SDK schema at
[commit 3ce13b6f334da16399ffc6c53c73a73366f728d6](https://github.com/linear/linear/blob/3ce13b6f334da16399ffc6c53c73a73366f728d6/packages/sdk/src/schema.graphql)
defines Query.viewer and User.organization. This query deliberately omits name,
email and profile URLs. The trusted worker accepts only a successful GraphQL
data response with the declared string ids and no errors, then submits sealed
identity to the [custody operation](./connector-account-identity.md). It does not
compute a guessable public digest. GraphQL errors/partial data, wrong types or
missing organization identity fail the probe.

## Response and quota policy

The exact bytes of [linear-response-policy-v1.json](/connector-fixtures/linear-response-policy-v1.json)
are digest-pinned by the definition's ResponsePolicyRef. Deployment must register
and verify an implementation of this policy; an unknown id, unavailable worker
or digest mismatch refuses admission. The JSON file is a declarative policy,
not a fabricated code artifact or proof that a filter is deployed.

Accept bounded JSON objects, reject duplicate members and excessive depth, and
scan header values/decoded JSON strings for the injected credential and the
declared finite encodings. Preserve permitted response bytes/status. This does
not prove detection of arbitrary malicious transformations by an upstream; a
provider/profile requiring that guarantee is unsupported.

API401/403 are observations, not proof of a dead OAuth grant or missing scope.
Preserve the provider error without logging its body. A trusted custody refresh
may establish invalid_grant for reconnect; it never repeats the original API
request. Recognize only the documented RATELIMITED code and declared rate fields;
unknown codes never widen authority or trigger automatic consent.

Use an initial shared-app safety budget of 4,000 requests/hour, then lower it
when response metadata or configured tenant shares require. It is deliberately
conservative, not an assertion that Linear has one global per-app quota. Provider
quota is actor-dependent and also has endpoint/complexity dimensions. Stop or
back off on the most restrictive applicable signal; never translate complexity
remaining into request remaining. The reset timestamp headers are UTC epoch
milliseconds, represented by RESET_AT_MILLISECONDS; Retry-After is parsed
separately. [Linear rate-limit documentation](https://linear.app/developers/rate-limiting)
is authoritative and must be rechecked before release.

## Runtime admission gate

Before enabling the release, verify the actual registered app/callback and actor,
minimal scope consent, probe schema, fresh/reconnect duplicate behavior, rotated
refresh with lost responses, revoke outcomes, policy-digest enforcement, provider
401/403/429/GraphQL errors, millisecond rate metadata and no credential leakage.
Record the app, release digest, policy digest and exact deployed build. Do not
simulate provider acceptance with the local JSON fixture. Existing inbound
webhooks stay on their legacy path until the separate ingress contract lands.
