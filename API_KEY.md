# API Key Design

## Status

This is the design of record for the API key platform. Nothing in it is
implemented: no bearer key, signed key, keyspace, or `ApiPrincipal` code
exists in the repository.

### What owns what

Four artifacts describe this system. Only one of them is this file.

```text
docs/adr/  0046, 0048, 0050, 0051, 0058, 0059, 0060, 0061
  -> the decisions. Where this file and an ADR disagree, the ADR wins.

proto/trogonai/platform/api_keys/**
proto/trogonai/platform/api_keyspaces/**
  -> the shapes: fields, types, enums, commands, events, state, and the
     authorization and admission contracts. Checked by buf.

CREDENTIAL_PLATFORM_SPEC.md
  -> the wire contract and the build order: endpoints, error codes,
     idempotency namespaces, the canonical signed-request target and its
     test vectors, UI acceptance.

this file
  -> the reasoning: why this shape and not another, what the design
     borrows and what it refuses, the ordered create and verification
     flows, the permission vocabulary, and the relationship to the
     secret store.
```

Field lists, enum members, and claim tables are deliberately absent below.
They used to be here and they drifted. This file said `public_key_pem`
while the proto says SPKI DER, carried a `last_used_at` the projection
deliberately omits and explains, declared a `product_surface` no proto has,
never mentioned the keyspace tier the events carry, and described `tgt` as
the request target when the wire contract makes it a digest of the target.

Prose about a shape has no build step that catches it going stale. So the
shapes live where `buf` checks them, and this file links rather than
restates. Adding a field list here is how the next five contradictions get
written.

What remains open is in [Open Questions](#open-questions).

## Purpose

This document defines how Trogonai should model API keys and how API-key
management relates to the broader secret-store design.

API keys are not all the same. The design must separate:

```text
provider API keys
  -> issued by another platform
  -> raw value needed later by Trogonai
  -> stored in OpenBao

Trogonai API keys
  -> issued by Trogonai
  -> only verification is needed
  -> verifier-only DB record
```

This distinction is the core rule.

## Design Decisions

- Provider API keys are credential material and belong in OpenBao.
- Trogonai-issued API keys are verifier-only records and should not be stored as
  decryptable secrets by default.
- API keys are shown once at creation or reroll time.
- API-key list/read APIs return metadata only.
- API keys authorize domain commands or runtime sessions; they do not directly
  grant raw secret access.
- API-key verification returns a domain principal with permissions, roles,
  project, identity, and allowed resource boundaries. There is no separate
  `scopes` vocabulary; permissions are the only one (ADR#0064).
- Rate limits, permissions, identities, lifecycle state, and audit events are
  part of the API-key domain model, not optional add-ons.
- OpenBao stores the verifier pepper, but not individual raw Trogonai API
  keys. The pepper is resolved once at process start and never touched on
  the request path (ADR#0062).
- Signed request keys are the strongly recommended mode for every caller,
  not a high-risk exception; bearer keys are the labeled compatibility tier
  for tooling that cannot sign (ADR#0050).
- Signed-request API keys store public verification material and metadata.
  The key pair is client-generated. The platform never generates,
  transmits, stores, or displays a private key (ADR#0050).

## Reference Inspiration

Unkey is the closest reference for platform-issued API keys. Useful concepts to
copy:

- keyspaces or API namespaces;
- root/management keys;
- ordinary API keys;
- roles and permissions;
- identities;
- per-key or identity-level rate limits;
- rerolling with grace periods;
- metadata-only list views;
- audit logs;
- usage analytics.

Do not copy Unkey as the storage model for provider API keys. Provider keys are
secrets Trogonai must recover later, so they belong behind the `SecretStore`
boundary and OpenBao.

[Coinbase App API Key Authentication](https://docs.cdp.coinbase.com/coinbase-app/authentication-authorization/api-key-authentication)
is the reference for high-security signed API keys. Useful concepts to copy:

- create keys with permission restrictions;
- attach IP allowlists where possible;
- use an asymmetric private/public key pair instead of a reusable bearer secret;
- generate a short-lived JWT for a specific request;
- include the key id in the JWT header;
- include the request method, host, and path in the signed payload;
- include validity bounds and a nonce to reduce replay risk;
- keep the private key out of code, logs, durable DB rows, and normal API
  responses.

Two things about the Coinbase reference are deliberately not copied.

Their portal generates the key pair server-side and hands the private key
back as a one-time download, which reintroduces the plaintext-issuance
moment the whole protocol exists to remove. ADR#0050 rejects it: key pairs
are client-generated only.

Their App API surface requires ES256/ECDSA and does not support Ed25519.
That is their compatibility constraint, not a security finding. ADR#0050
makes Ed25519 the default and accepts ES256.

The binding set is also stronger here than the Coinbase sketch. ADR#0051
adds a payload digest, a `jti` checked against a replay store, DPoP-style
server-issued nonces, and a NATS subject mapping for the internal
transport. Tokens are single-use; there is no multi-use or reduced-binding
mode.

The two modes are not peers, and the platform does not present them as a
choice between equals (ADR#0050):

```text
Signed key (recommended for every caller)
  -> the platform stores a public key, which is not a secret
  -> nothing replayable crosses the wire
  -> the request is bound to target, payload, and a validity window
  -> every request is attributable to a specific key by signature
  -> rotation is a public-key swap

Bearer key (compatibility tier)
  -> for tooling that cannot sign requests
  -> verifier-only DB record, one-time display
  -> easiest SDK and CLI experience
  -> the raw secret is on the wire on every request, so every proxy, log
     line, and TLS-terminating hop sees a replayable credential
  -> keyspace policy can disallow it entirely
```

## Enterprise Direction

The enterprise-ready direction should stay between the Unkey and Coinbase
styles:

```text
Unkey-style control plane
  -> keyspaces
  -> identities
  -> root and ordinary keys
  -> permissions and roles
  -> rate limits
  -> rerolling and revocation
  -> metadata-only list/read APIs
  -> audit and usage analytics

Coinbase-style authentication proof
  -> asymmetric private/public key pair, client-generated
  -> private key held by caller, never by the platform
  -> public key registered with Trogonai
  -> short-lived single-use signed request token
  -> target, payload digest, validity, and jti binding
  -> server-issued nonce escalation
  -> optional IP allowlist
```

Do not frame the first serious version as "Unkey versus Coinbase." Use Unkey
for the platform management model and Coinbase for the high-security
authentication model.

The split is by keyspace policy, not by how important the key feels
(ADR#0050 section 5, ADR#0063 section 3):

```text
management keyspace
  -> signed only; bearer issuance disallowed
  -> server nonce required
  -> holds root and management keys

default keyspace
  -> signed recommended, bearer permitted
  -> server nonce optional
  -> holds ordinary product API keys
```

This gives Trogonai an enterprise-shaped API key system without starting with a
full generic OAuth product. The design should stay compatible with future
OAuth-style enterprise integrations, but the product surface for the first
version should be the API-key system users can understand:

```text
Create keyspace
Create identity
Register a signed key, or create a bearer key where policy allows
Assign roles / permissions / limits / allowlists
Verify requests
Audit usage
Rotate or revoke
```

### Enterprise Version To Focus On First

Focus on this first:

```text
Trogonai API Key Platform
  -> Unkey-style management and policy
  -> Coinbase-style signed high-authority keys
  -> OpenBao for provider credentials and platform verifier material
```

This is the pragmatic enterprise version because it gives customers:

- signed keys as the recommended path for all automation;
- bearer keys as a labeled compatibility tier where tooling cannot sign;
- scoped root keys for management APIs;
- identities that group multiple keys under a user, team, service, or agent;
- permissions and built-in roles that are visible in the UI;
- rate limits and abuse controls;
- IP allowlists for sensitive keys;
- short-lived single-use signed request tokens;
- audit trails for create, deny, reroll, revoke, public-key add and revoke,
  and signed-request replay;
- metadata-only key list views;
- no caller private keys stored, ever;
- no provider secrets in the normal application database.

What not to focus on first:

- a general-purpose OAuth authorization server;
- FAPI certification;
- mandatory mutual TLS for every customer;
- customer-authored roles (built-in roles only, ADR#0064 section 2);
- identity-level shared rate limits (ADR#0064 section 6);
- certificate-based client authentication (JWT request tokens first);
- exposing OpenBao paths or policies as the product API.

Storing customer private keys is not a deferred item. ADR#0050 forbids it
outright; there is no configuration under which the platform holds one.

Those can be future compatibility paths. They should not define the first
version.

## API Key Categories

### Provider API Keys

Provider API keys are issued by another service and used by Trogonai later.

Examples:

- GitHub tokens;
- Slack tokens;
- Linear tokens;
- MCP server bearer tokens;
- LLM provider keys;
- other outbound API credentials.

Storage:

```text
Credential metadata
  -> application database

raw provider key
  -> OpenBao

runtime use
  -> CredentialDeliveryPolicy
```

Provider API keys should carry delivery policy:

```text
CredentialDeliveryPolicy
  allowed_environments
  allowed_runtime_services
  allowed_hosts
  injection_locations
```

Typical policy:

```text
Bearer provider token
  -> allowed_runtime_services: provider connector
  -> allowed_hosts: provider API hosts
  -> injection_locations: request headers
```

### Trogonai API Keys

Trogonai API keys are issued by this platform to users, automation, services, or
agents so they can call Trogonai APIs.

Storage:

```text
raw bearer key
  -> shown once
  -> discarded

verifier digest + verifier_key_version
  -> application database

verifier pepper
  -> OpenBao KV; resolved once at process start, held in memory,
     never touched on the request path (ADR#0062 section 3)

signed key private material
  -> nowhere. The caller holds it; the platform never sees it.
```

The platform should never need to recover a Trogonai API key in plaintext.
This is true even when Trogonai's own API key system generated the key. The
system creates the raw value, shows it once, derives verifier material, and then
forgets the raw value.

So yes, the API-key record lives in the normal application database. What must
not live there is the raw API key value.

```text
normal DB may store
  -> key id
  -> verifier digest and its verifier_key_version
  -> public key / certificate metadata for signed keys
  -> permissions / roles / rate limits
  -> project / identity / lifecycle metadata
  -> usage and audit metadata

normal DB must not store
  -> raw API key
  -> reusable bearer token value
  -> private key material for signed keys
  -> the verifier pepper
  -> provider secret material
```

The verifier digest is a value in the DB, but it is not the API key. It should
only be useful for checking a presented key. If the DB leaks, the attacker should
not be able to call the API with the stored digest.

## Data Model

The model is `proto/trogonai/platform/api_keys/**` and
`proto/trogonai/platform/api_keyspaces/**`. Start at `state/v1/state.proto`
in each, which carries the aggregate's full retained shape, then read the
commands and events in `v1/`.

What is worth knowing before reading it is what the model deliberately does
not have, because each absence answers a question that would otherwise be
reopened by habit:

- **No `scopes`.** Permissions are the only authorization vocabulary
  (ADR#0064 section 1). Two grant vocabularies mean every future check has
  to decide which wins when they disagree.
- **No stored `public_prefix`.** It is the derived display form
  `tg_<env>_<key_id>`, and lookup uses the key id directly, so there is no
  second minted value that can disagree with the first (ADR#0062 section 1).
- **No standalone `fingerprint`.** Signed keys fingerprint their public key;
  a bearer key has nothing to fingerprint that is not either the id or the
  digest.
- **No `owner_id`.** The owner boundary is `project_id` (ADR#0046).
- **No `last_used_at`.** It would put a write on the authenticated request
  path for the same reason `api_key.verified` is a metric rather than an
  audit fact. If it is wanted, derive it from the verification metric at a
  coarse granularity.

Resource boundaries are a `ResourceScope` rather than a bare list, so the
empty-list question never has to be answered at runtime (ADR#0064 section 4):
the scope is either the whole project or an explicit non-empty set, and an
empty set is a validation error rather than a value with two possible
readings.

The two credential tiers are distinguished structurally rather than by a
`kind` discriminator that a caller could set inconsistently with the rest of
the record. A bearer key carries a verifier digest and its pepper version; a
signed key carries one or two registered public keys and never anything
private. The create command's oneof is shaped so that "a signed key whose
private half the platform generates" cannot be expressed at all, which is
the request ADR#0050 section 2 exists to refuse.

## Key Format

The format, its components, and the scanner-facing pattern are fixed in
ADR#0062 section 1.

```text
tg_<env>_<key_id>_<secret><checksum>
```

Two properties of it matter to the rest of this document. The `<env>`
segment stays truthful because keyspace environment is immutable
(ADR#0063 section 2), so a key minted in a test keyspace cannot come to
display as live. And the checksum lets SDKs and secret scanners reject
typos and truncations offline, before anything reaches verification.

Do not rely on the prefix for authorization. Authorization comes from the DB
record after verification.

Signed keys have no wire-format secret and therefore no `<secret>` or
`<checksum>` component. They share the identifier, not the format: the
`kid` in the request token header is the same `ApiKeyId`, so key lookup is
one code path for both tiers.

### Signed Request Token

The binding contract is ADR#0051. The claim set, its canonical
serialization, and the canonicalization test vectors are in
[CREDENTIAL_PLATFORM_SPEC.md](./CREDENTIAL_PLATFORM_SPEC.md); the message
shape is `api_keys/admission/v1/signed_request.proto`. Tokens are
single-use; there is no multi-use or reduced-binding mode.

The target binding is what stops a captured signature from being replayed
against a different endpoint. A token signed for
`GET api.trogon.ai/v1/projects/p_123` must not authorize
`POST api.trogon.ai/v1/projects/p_123/api-keys`. The target is transport
mapped:

```text
HTTP   method, authority, path, and query
NATS   the concrete subject the message was published on, plus the
       operation in the payload envelope
```

One canonical serialization covers both, fixing case, default-port elision,
and percent-encoding for HTTP targets. It ships with canonicalization test
vectors for both transports and lands before signed verification does,
because a target the client and server serialize differently is a
signature that fails for reasons neither side can see.

The payload digest is always required. Without it a token authorizes any
body sent to that target inside its window, and the `jti` only stops the
second use, not the first substitution. Bodyless requests use a defined
constant digest rather than omitting it, so "no body" and "body not
covered" cannot look alike on the wire.

Subject mapping that rewrites NATS subjects breaks signatures the same way
a rewriting HTTP proxy does, and is unsupported in front of signed
subjects.

This is asymmetric signing, not mutual TLS. A later certificate mode could
store an X.509 chain and validate client certificates at the edge; the
first signed-key mode is JWT-based because it is easier for SDKs and maps
closely to the Coinbase reference. Certificate mode is a future extension,
not a first-version item.

## Create Flow

### Bearer Key Create Flow

```text
1. Reject if keyspace policy disallows bearer issuance.
2. Authorize caller to create an API key in the target keyspace, and check
   the delegation ceiling against the caller's own effective permissions.
3. Mint the ApiKeyId.
4. Generate 32 bytes of CSPRNG secret; assemble the wire format.
5. Compute verifier_digest = HMAC-SHA-256(pepper[v], domain-separated
   key_id and secret); record v as verifier_key_version.
6. Store API key metadata and verifier digest in the database.
7. Emit api_key.created audit event without raw key material.
8. Return raw key once.
9. Discard raw key.
```

Step 1 comes first on purpose. A caller who is authorized to create keys in
a signed-only keyspace should be told the tier is unavailable, not have a
bearer secret minted and then rejected on policy.

The raw key appears in the response only on the first, non-replay create or
reroll. Every later response is metadata-only, including an idempotent
replay of the same call by the same caller (ADR#0048).

### Signed Key Create Flow

There is one flow. Client-generated key pairs are the only mode (ADR#0050
section 2).

```text
1. Authorize caller to create a signed API key in the target keyspace, and
   check the delegation ceiling.
2. Client generates an asymmetric key pair locally.
3. Client submits the public key plus metadata. Nothing else.
4. Server validates algorithm against keyspace allowed_algorithms, key
   encoding and strength, project, permissions, and policy.
5. Server stores public verification material, fingerprint, and metadata.
6. Server emits api_key.created audit event.
7. Client keeps the private key locally.
```

There is no server-generated fallback. The platform never generates,
transmits, stores, or displays a private key, so the ADR#0048 one-time
display machinery does not apply to this tier at all: no secret ever exists
on the platform side to display once. An earlier draft of this document
described a server-generated one-time download; that flow is superseded and
must not be built.

This is also why signed keys have the cleaner retry story. A repeated
registration of the same public key is a metadata-only replay with nothing
to lose, so a client that loses the response has lost nothing.

Storing a caller's private key in OpenBao is not a deferred option either.
It would make the platform custodian of a long-lived signing key, which is
the exact property signed mode exists to remove. OpenBao holds the
platform's own material: the verifier pepper, issuer signing keys, and
certificate authority keys if Trogonai later runs a certificate-issuing
flow.

## Verification Flow

### Bearer Key Verification Flow

```text
1. Parse the key id from the wire format; verify the checksum and reject
   malformed input before any lookup.
2. Load API key metadata, verifier digest, and verifier_key_version.
3. Reject if keyspace, project, environment, status, expiry, or revocation
   fails.
4. Compute the verifier digest for the presented secret under the pepper
   version recorded on the key.
5. Compare in constant time.
6. Evaluate effective permissions, resource scope, and requested operation.
7. Evaluate rate limits.
8. Record non-secret usage metadata; emit api_key.denied on failure.
9. Return authenticated principal.
```

### Signed Key Verification Flow

```text
1. Parse `kid` from the JWT header.
2. Load API key metadata and public verification material.
3. Reject if keyspace, project, environment, status, expiry, or revocation
   fails.
4. Reject an algorithm outside keyspace allowed_algorithms, or a missing
   signed-request policy.
5. Verify the JWT signature against the stored public key.
6. Validate issuer, subject, iat, exp, and clock skew (at most 30 seconds,
   window at most 2 minutes).
7. Atomically check-and-record (key_id, jti) in the replay store as a
   single conditional create. Key-already-exists is the replay rejection.
   Fail closed if the store cannot be consulted.
8. Validate the server nonce where keyspace policy requires it; reject with
   a fresh nonce challenge if absent or stale.
9. Validate the target binding against the canonical serialization for the
   transport, and the payload digest against the exact request body.
10. Evaluate effective permissions, resource scope, and requested operation.
11. Evaluate rate limits.
12. Record non-secret usage metadata; emit api_key.denied on failure and
    api_key.signed_request_replayed on a replay rejection.
13. Return authenticated principal.
```

Step 7 is a single atomic conditional create, never a read followed by a
write, because the read-then-write form is a race that admits the replay it
exists to stop. Replay records live for the validity ceiling plus twice the
clock-skew tolerance: skew is bidirectional, so a token admitted at the
earliest tolerated moment stays verifiable until expiry plus skew, and the
record has to outlive that whole span.

Verification happens once, at admission, by the first service that accepts
the request while the token is fresh. Downstream and JetStream consumers do
not re-verify caller tokens; authority travels onward as provenance
(ADR#0039). An expired token inside a persisted message is the expected
state of an already-admitted request, not an error.

A target-binding failure at step 9 is the one verification failure that
returns a detail: the verifier's own computed canonical target, so an SDK
author can diff it against theirs. It is emitted only after step 5 has
succeeded against a live public key, never for an unknown, revoked, or
expired key, because a caller who reaches step 9 holds the private key and
learns only how the server serialized their own request. Every other
failure returns a bare code. The vocabulary is in
[CREDENTIAL_PLATFORM_SPEC.md](./CREDENTIAL_PLATFORM_SPEC.md).

Verification produces an `ApiPrincipal`
(`api_keys/admission/v1/api_principal.proto`). Authorization checks read its
effective permission set and never recompute the union. Roles and direct
permissions ride along because audit and the UI have to explain why a
decision went the way it did, and a bare effective set cannot answer that.

The principal can authorize a command or session. It must not directly fetch raw
credential material.

## Permissions And Roles

Permissions are strings, not an enum, so the vocabulary lives here rather
than in the proto. Use them for concrete capabilities:

```text
api_keys.verify
api_keys.create
api_keys.reroll
api_keys.revoke
credential_vaults.read
credential_vaults.use
integrations.create
integrations.rotate
gateway.sessions.create
```

Roles group permissions. The first milestone ships built-in roles only;
customers assign them and do not author them (ADR#0064 section 2). The set
and what each one covers is `api_keys/v1/role.proto`.

API keys may have roles and direct permissions:

```text
effective_permissions =
  union(permissions of assigned roles) union direct_permissions
```

Direct permissions are the bounded escape hatch. They may only add
permissions the assigning actor already holds, they render as a distinct
UI badge rather than folding into the role display, and keys carrying them
are marked in list views. That turns "they make audits harder" from advice
nobody reads into something visible to the person granting them.

Resource scope is a separate, ANDed dimension. `allowed_vaults`,
`allowed_integrations`, `allowed_environments`, and the runtime-side
`allowed_runtime_services` and `allowed_hosts` narrow where a permission
applies; they never grant.

The delegation ceiling is a checked invariant, not a guideline. Create and
patch fail with `permission_denied` when the resulting key's effective
permissions are not a subset of the acting principal's, or when its
resource scope is not contained in the acting principal's scope.
`api_keys.create` confers the ability to create a key, never the ability to
grant an authority the creator lacks. Root keys are not exempt; a root
key's ceiling is simply the whole project, so the same check runs and
passes.

## Root And Management Keys

Root and management keys live in the project's `management` keyspace, which
is signed-only and demands server nonces by policy (ADR#0063 section 3).
The separation is enforced by keyspace policy rather than by convention.

```text
Root key
  -> management API access
  -> can create, update, reroll, revoke, or verify keys according to
     permissions, bounded by the delegation ceiling
  -> signed only; bearer issuance is disallowed in this keyspace

Ordinary key
  -> product/API access
  -> scoped to specific operations and resources
```

Root keys are:

- rare;
- environment-specific;
- permissioned;
- rotated periodically, on the shorter management grace (ADR#0065 section 2);
- forbidden from browser/client-side use;
- audited aggressively;
- signed-request keys always, not by default.

### Bootstrap

Self-hosted deployments start with no key at all, and creating a key
requires a key. The bootstrap is a public key supplied as deployment
configuration (ADR#0063 section 4):

```text
1. The operator generates a key pair locally with the CLI.
2. Only the public key is supplied to the platform as configuration.
3. On first boot the platform registers it as a root signed key in the
   project's management keyspace.
4. The private key never leaves the operator's machine.
```

Registration is idempotent by public-key fingerprint, so re-applying the
same configuration is a no-op and the flow is safe under a declarative
apply. If a root key already exists under a different fingerprint, first
boot does not replace it.

Printing an initial root token at first boot, the way `bao operator init`
does, is rejected: it would mint a bearer key into a signed-only keyspace
and reintroduce the plaintext-issuance moment at the highest-authority
credential the deployment will ever have.

Cloud has no separate bootstrap. The first root key there is registered by
an authenticated human session against the human identity system
(ADR#0053), not by an API key.

Losing every root private key with no break-glass host access is
unrecoverable by design. That is the correct trade for a platform whose
central promise is that it never holds a caller's private key.

## Rate Limits

The policy shape and the enforced subject kinds are
`api_keys/v1/rate_limit_policy.proto`. The first milestone enforces `key`
and `project` only; `identity` and `route` are declared and not enforced
(ADR#0064 section 6).

Identity-level shared limits are deferred because they need a counter
shared across keys, which is the one shape that turns a per-key local
decision into cross-key contention on the admission path. There is no
shipped identity concept to attach one to yet.

Counters live in NATS JetStream KV with revision-checked updates, alongside
the replay store, so the first milestone adds no infrastructure dependency.
The ceiling is real: compare-and-set on a per-key record bounds write
throughput per key, so the limiter is coarse. The trigger for moving to a
dedicated counter store is sustained CAS retry rates on the admission path,
or a configured per-key limit high enough that the counter becomes the
bottleneck before the limit does.

Failure behavior splits by what the limit protects:

```text
ordinary keyspaces    fail open when the counter store is unavailable
management keyspaces  fail closed
```

A limiter on product traffic is a fairness control, and its outage should
not become an API outage. On a management keyspace the same mechanism is an
abuse control on privileged operations. Both paths emit a distinct metric,
so a fail-open window is visible rather than silent. This is deliberately
unlike the replay store, which fails closed in every case, because replay
protection is a security control and rate limiting mostly is not.

Verification returns rate-limit context so callers can set response headers
and logs without leaking secrets.

Apply rate limits to:

- public API calls;
- credential verification attempts;
- management API writes;
- gateway session creation;
- integration registration calls.

## Rerolling

Reroll is planned rotation. Revoke is compromise. Keeping them separate is
what makes a non-zero grace period safe: nobody has to choose between a
rotation that will not break their fleet and a containment action that
takes effect now (ADR#0065).

Rerolling creates a new raw key while preserving the old key's policy. It
is bearer-only.

```text
old key
  -> remains valid for grace period
  -> shown as `rotating` with its expiry, not as `active`

new key
  -> fresh secret
  -> inherits metadata, permissions, roles, identity, and rate limits
  -> shown once

after grace
  -> old key revoked
```

Showing the old key as `rotating` rather than `active` is what makes a
rotation somebody started and forgot to finish visible in the product
instead of only in the data.

Grace defaults and the configurable range are ADR#0065 section 2, carried
on keyspace policy. Ordinary keyspaces get a default long enough to cover a
deploy cycle for consumers the platform has no visibility into; management
keyspaces have few consumers and privileged reach, so they get a much
shorter one. A grace of 0 is legal and means the old key stops verifying
the moment the new one is issued.

Expired or revoked keys should not be rerollable unless a recovery workflow
explicitly allows it.

Signed keys use public-key rotation instead of bearer rerolling:

```text
old public key
  -> remains accepted while both keys are active

new public key
  -> inherits metadata, permissions, roles, identity, and rate limits
  -> private key stays with the caller

after cutover
  -> old public key revoked
```

The window is bounded by `max active public keys per signed key`, fixed at
2, which makes it a rotation window rather than an accumulating set. The
bearer grace-period machinery does not apply here.

The rotation flow must never require the caller to upload a private key.

## Revocation And Expiry

Revocation is logical first:

```text
1. Mark key revoked in DB.
2. Emit api_key.revoked audit event.
3. Verification rejects the key immediately.
4. Cleanup/retention jobs handle old metadata later.
```

Expiry should be checked during verification. Expired keys should fail closed.

Do not physically delete key metadata immediately. Keep tombstones for audit and
support.

## Interaction With Credential Vaults

API keys can authorize access to credential vault commands or sessions, but they
must not expose raw provider credentials.

```text
API key verifies
  -> produces ApiPrincipal
  -> domain authorizes command/session
  -> runtime resolves CredentialRefs if allowed
  -> OpenBao returns raw provider credential only to trusted runtime service
```

Resource scopes:

```text
allowed_vaults            on the key
allowed_integrations      on the key
allowed_environments      on the key
allowed_runtime_services  on the credential's delivery policy
allowed_hosts             on the credential's delivery policy
```

The last two already exist and are enforced in the runtime projection; the
first three land with the API key platform. All five are ANDed, and none of
them grants anything on its own.

Avoid broad project-wide API keys that can use every vault. A project-wide
resource scope exists because some keys legitimately need it, not because it
is a sensible default, and a key that holds it should be as rare as a root
key.

## Security Requirements

- Never log raw API keys.
- Never put raw API keys in NATS messages, outbox messages, metrics, traces,
  URLs, workflow state, or durable saga payloads.
- Store only verifier digests for Trogonai-issued API keys.
- Store only public verification material for signed Trogonai API keys.
- Compare verifier digests in constant time.
- Verify signed-request tokens against target, payload digest, validity
  window, and replay store before authorization.
- Fail closed when the replay store cannot be consulted.
- Return raw keys only once, on the first non-replay execution.
- Redact presented keys in validation errors and audit events.
- Record non-secret usage metadata such as key id, project, operation,
  result, timestamp, and source address.
- Keep the verifier pepper out of the API-key table, and out of every
  event, snapshot, projection, log, metric, and trace.
- Keep signed-key private material out of the platform entirely, not merely
  out of the API-key table.

Redaction is structural, through `SecretString` and its peers, rather than
a formatting rule. The existing no-plaintext-in-logs and
no-plaintext-in-traces tests pass because the redaction lives in the `Debug`
implementations and cannot be bypassed by a different exporter; API key
material inherits that property rather than reinventing it.

## Observability

Emit metrics and logs for operations, not values:

- key verification count by keyspace and result;
- permission denial count;
- rate-limit denial count, and rate-limiter fail-open windows;
- revoked/expired key attempts;
- reroll count;
- revoke count;
- create count;
- usage by key id or fingerprint if cardinality has been reviewed.

The required first-release audit facts are fixed at seven in ADR#0065
section 5 and modelled in `api_keys/audit/v1/audit_fact.proto`. Each is
either a change of authority or an attack signal. Two of them,
`api_key.denied` and `api_key.signed_request_replayed`, are the signals the
two unfirable alerts in `devops/openbao/runbooks/alerts.md` are waiting on.

Two things that look like audit facts and are not:

```text
api_key.verified   a metric, not an audit fact. Its volume equals the
                   platform's authenticated request rate, which would make
                   audit the highest-volume stream on the platform and put
                   its retention and cost profile on the request path.
                   Attribution comes from event provenance (ADR#0039).

api_key.expired    emitted lazily on the first denied use, not by a
                   sweeper. Expiry is a property of the record, and a
                   sweeper would emit facts about keys nobody was using.
```

Every audit fact carries key id, keyspace id, project id, actor, result,
timestamp, and source address, and never the presented key, the request
token, the verifier digest, or the pepper. The one legitimate unattributed
fact is the bootstrap `api_key.created`, which has a null actor because no
authenticated actor exists at that moment.

## API Surface

The wire contract lives in
[CREDENTIAL_PLATFORM_SPEC.md](./CREDENTIAL_PLATFORM_SPEC.md), section
API_CONTRACTS, alongside every other endpoint on the platform: the
project-anchored route list (ADR#0046 section 3), the signed and bearer
create variants, the reason there is no public verify route, per-endpoint
idempotency namespaces, the error vocabulary, and the canonical
signed-request target serialization.

It is there rather than here because splitting the endpoint table across
two documents is how the two copies drift.

## Idempotency And Retry Safety

API-key management endpoints should be idempotent, especially create, reroll,
revoke, and public-key registration. A client retry must not create unlimited
dead keys.

The client provides an opaque idempotency key per logical user action. The
server scopes it after authentication. The record shape is
`api_keys/idempotency/v1/idempotency.proto`; the per-endpoint namespaces are
in [CREDENTIAL_PLATFORM_SPEC.md](./CREDENTIAL_PLATFORM_SPEC.md).

Uniqueness is scoped by project, so two different companies can use the same
idempotency key safely:

```text
unique(project_id, command_namespace, idempotency_key)
```

For commands that target an existing key, the target joins the scope:

```text
unique(project_id, command_namespace, target_resource_id, idempotency_key)
```

The contract is:

```text
same project + same namespace + same key + same request fingerprint
  -> return the same operation/resource/status

same project + same namespace + same key + different request fingerprint
  -> reject with idempotency conflict

different project + same namespace + same key
  -> unrelated operation
```

The idempotency key is not an authorization token. Every retry must still
authenticate and authorize normally. By default, response replay requires
the same actor/client that created the idempotency record or an actor with
explicit permission to view the target key.

The response snapshot is metadata-only. It may carry the operation id, key
id, derived public prefix, public key fingerprint, status, and validation
errors. It must never carry a raw bearer key or a reusable token value.
Generated private keys are absent from that prohibition because they no
longer exist anywhere: the platform never generates one (ADR#0050).

This creates the product rule for one-time display values.

If a bearer API key was created successfully but the client lost the
response, the backend must not redisplay the raw value. It returns metadata
and requires a reroll.

DECIDED (ADR#0048): there is no server-side escrow of one-time material, in
any form. A replay of the same idempotency key returns metadata only, never
plaintext, even to the caller who originally received it on the first,
non-replay execution. Recovery from a lost one-time response is reroll, never
recovery of the original value. An earlier draft of this document floated a
short-lived escrow behind the secret-store boundary; that option is closed,
not deferred.

Signed keys have a cleaner retry story, which is one more reason they are
the recommended tier:

```text
client-generated signed key
  -> client keeps private key
  -> retry registers the same public key
  -> response replay is metadata-only
  -> a lost response costs nothing
```

Use pending-operation limits to prevent accidental or malicious row growth:

```text
max pending api_key.create operations per project
max pending api_key.reroll operations per key
max active root keys per project
max active public keys per signed key    fixed at 2 (ADR#0065 section 4)
```

## Open Questions

- Which identity kinds ship first (user, team, service, agent), and whether
  identity is required or optional on a key in the first milestone.
- Whether the internal `/-/credentials` admin API is replaced by API-key
  authentication or retired outright when the public surface lands. It has
  no per-owner authorization check today, so it does not survive contact
  with a real principal unchanged either way.
- Concrete first-milestone values for the pending-operation caps above.
- Whether SDKs verify the wire-format checksum before sending, which would
  keep typo'd keys out of the verification failure metrics entirely.
- Whether `api_key.policy_changed` needs its own patch endpoint in the
  first release, or whether policy is create-time only to start.

Questions that are closed are closed in the ADRs, and
[docs/adr/index.md](./docs/adr/index.md) is the list. This file does not keep
a second one.

## Relationship To Secret Store

The final split is:

```text
Trogonai API key
  -> verifier-only DB record
  -> authenticates caller
  -> authorizes domain command/session

Provider credential
  -> raw material in OpenBao
  -> DB CredentialRef metadata
  -> gateway/runtime resolves only when authorized
```

API-key authentication is part of the control plane. Secret storage is part of
the credential material plane. They interact, but they should not collapse into
one generic "secret" abstraction.
