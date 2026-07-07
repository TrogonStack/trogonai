# API Key Design

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
- API-key verification should return a domain principal with scopes, roles,
  owner, identity, and allowed resource boundaries.
- Rate limits, permissions, identities, lifecycle state, and audit events are
  part of the API-key domain model, not optional add-ons.
- OpenBao may store a verifier pepper or signing secret, but not individual raw
  Trogonai API keys.
- High-risk platform API keys should support an asymmetric signed-request mode,
  not only reusable bearer strings.
- Signed-request API keys should store public verification material and
  metadata, while the private key is generated client-side or shown once and
  discarded.

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
- include `nbf`, `exp`, and a nonce to reduce replay risk;
- keep the private key out of code, logs, durable DB rows, and normal API
  responses.

Coinbase's Coinbase App docs currently require ES256/ECDSA for that API
surface and warn that Ed25519 is not supported there. For Trogonai, the
important pattern is not the exact Coinbase compatibility constraint. The
important pattern is asymmetric request signing with short-lived request tokens.

This is not strictly better or worse than Unkey. It solves a different class of
risk.

```text
Unkey-style bearer key
  -> best for simple developer API keys
  -> easiest for users and SDKs
  -> strong lifecycle, metadata, permissions, rate limits, analytics

Coinbase-style signed key
  -> best for root keys, management keys, financial/high-risk operations,
     agent infrastructure, and service-to-service automation
  -> private key is not sent on each request
  -> request is bound to method/host/path/time
  -> more secure, but more complex for users and SDKs
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
  -> asymmetric private/public key pair
  -> private key held by caller
  -> public key registered with Trogonai
  -> short-lived signed request token
  -> method/host/path binding
  -> nonce and expiry
  -> optional IP allowlist
```

Do not frame the first serious version as "Unkey versus Coinbase." Use Unkey
for the platform management model and Coinbase for the high-security
authentication model.

```text
ordinary developer key
  -> Unkey-style bearer key
  -> verifier-only DB record
  -> easiest SDK and CLI experience

root / management / high-risk key
  -> Coinbase-style signed key
  -> public verification material in DB
  -> private key never stored by default
  -> request-specific signed token
```

This gives Trogonai an enterprise-shaped API key system without starting with a
full generic OAuth product. The design should stay compatible with future
OAuth-style enterprise integrations, but the product surface for the first
version should be the API-key system users can understand:

```text
Create keyspace
Create identity
Create bearer key or signed key
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

- simple bearer keys for normal automation;
- strong signed keys for privileged automation;
- scoped root keys for management APIs;
- identities that group multiple keys under a user, team, service, or agent;
- permissions and roles that are visible in the UI;
- rate limits and abuse controls;
- IP allowlists for sensitive keys;
- short-lived signed request tokens for high-risk operations;
- audit trails for create, verify, deny, rotate, reroll, and revoke;
- metadata-only key list views;
- no raw caller private keys stored by default;
- no provider secrets in the normal application database.

What not to focus on first:

- a general-purpose OAuth authorization server;
- FAPI certification;
- mandatory mutual TLS for every customer;
- storing customer private keys in OpenBao as the default;
- exposing OpenBao paths or policies as the product API.

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
raw key
  -> shown once
  -> discarded

verifier digest
  -> application database

verifier pepper/secret
  -> OpenBao or key-management backend
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
  -> public prefix
  -> verifier digest
  -> public key / certificate metadata for signed keys
  -> scopes / roles / rate limits
  -> owner / identity / lifecycle metadata
  -> usage and audit metadata

normal DB must not store
  -> raw API key
  -> reusable bearer token value
  -> private key material for signed keys
  -> provider secret material
```

The verifier digest is a value in the DB, but it is not the API key. It should
only be useful for checking a presented key. If the DB leaks, the attacker should
not be able to call the API with the stored digest.

## Data Model

Suggested model:

```text
ApiKey
  id
  keyspace_id
  owner_id
  identity_id
  kind
  display_name
  public_prefix
  verifier_digest
  public_key_pem
  public_key_fingerprint
  signing_algorithm
  signed_request_policy
  fingerprint
  scopes
  roles
  direct_permissions
  rate_limit_policy_refs
  allowed_vaults
  allowed_integrations
  allowed_environments
  created_by
  created_at
  expires_at
  revoked_at
  last_used_at
  last_used_from
  rerolled_from
  rerolled_to

ApiKeySpace
  id
  owner_id
  name
  environment
  product_surface

Identity
  id
  owner_id
  kind
  external_ref
  metadata
```

`public_prefix` is used for lookup and support. It must not be enough to
authenticate. `fingerprint` is non-secret and exists for support, audit, and UI
display.

`kind` separates verifier-backed bearer keys from asymmetric signed keys:

```text
ApiKeyKind::Bearer
  -> stores verifier_digest
  -> raw key is shown once and discarded

ApiKeyKind::Signed
  -> stores public_key_pem or certificate chain
  -> stores public_key_fingerprint
  -> stores signing_algorithm
  -> private key is never stored by default
```

If Trogonai generates the private key for a signed key, it should return it once
and discard it. Prefer client-generated key pairs when the user or SDK can
handle that flow because the private key never crosses Trogonai infrastructure.

## Key Format

The exact format can change, but it should support:

- a product prefix;
- an environment marker when useful;
- a public lookup prefix or key id;
- a high-entropy secret part.

Example shape:

```text
tg_live_<public_id>_<secret>
tg_test_<public_id>_<secret>
```

Do not rely on the prefix for authorization. Authorization comes from the DB
record after verification.

Signed keys use the key id for lookup and a short-lived request token for
authentication. A Coinbase-inspired shape:

```text
Authorization: Bearer <signed_jwt>

JWT header:
  kid: <api_key_id>
  alg: ES256
  nonce: <random_nonce>

JWT claims:
  iss: trogonai
  sub: <api_key_id>
  nbf: <now>
  exp: <now + short_window>
  uri: <METHOD> <HOST><PATH>
```

The `uri` claim binds the token to the request. A token signed for
`GET api.trogon.ai/v1/projects` must not authorize
`POST api.trogon.ai/v1/api-keys`.

This is asymmetric signing, not necessarily mutual TLS. A later certificate
mode can store an X.509 certificate chain and validate client certificates at
the edge, but the first signed-key mode can be JWT-based because it is easier
for SDKs and maps closely to the Coinbase reference.

## Create Flow

### Bearer Key Create Flow

```text
1. Authorize caller to create an API key in the target keyspace.
2. Generate a high-entropy random secret.
3. Generate a non-secret public prefix / lookup id.
4. Compute verifier digest using the presented key and verifier pepper.
5. Store API key metadata and verifier digest in the database.
6. Emit api_key.created audit event without raw key material.
7. Return raw key once.
8. Discard raw key.
```

Response should include the raw key only on create or reroll:

```text
{
  "id": "key_123",
  "public_prefix": "tg_live_abcd",
  "key": "tg_live_abcd_secret-shown-once",
  "display_name": "CI deploy key",
  "scopes": ["integrations.read"],
  "created_at": "..."
}
```

Every later response must be metadata-only.

### Signed Key Create Flow

Preferred client-generated flow:

```text
1. Authorize caller to create a signed API key in the target keyspace.
2. Client generates an asymmetric key pair.
3. Client submits public key or certificate signing request plus metadata.
4. Server validates algorithm, key strength, owner, permissions, and policy.
5. Server stores public verification material and metadata in the database.
6. Server emits api_key.created audit event without private key material.
7. Client keeps private key locally.
```

Fallback server-generated flow:

```text
1. Authorize caller to create a signed API key in the target keyspace.
2. Server generates an asymmetric key pair.
3. Server stores only public verification material and metadata.
4. Server returns private key once through a one-time response or download.
5. Server discards private key.
```

Do not store the generated private key in OpenBao by default. Storing it would
turn the platform into a custodian for a caller's long-lived signing key. Use
OpenBao for the platform's verifier pepper, issuer signing keys, or certificate
authority keys if Trogonai later runs a certificate-issuing flow.

## Verification Flow

### Bearer Key Verification Flow

```text
1. Parse public prefix / key id.
2. Load API key metadata and verifier digest.
3. Reject if keyspace, owner, environment, status, expiry, or revocation fails.
4. Compute verifier digest for the presented key.
5. Compare in constant time.
6. Evaluate scopes, roles, direct permissions, and requested operation.
7. Evaluate rate limits.
8. Record non-secret usage metadata and audit event.
9. Return authenticated principal.
```

### Signed Key Verification Flow

```text
1. Parse `kid` from the JWT header.
2. Load API key metadata and public verification material.
3. Reject if keyspace, owner, environment, status, expiry, or revocation fails.
4. Reject unsupported algorithm or missing signed-request policy.
5. Verify JWT signature against the stored public key.
6. Validate issuer, subject, not-before, expiry, and clock skew.
7. Validate nonce or replay policy.
8. Validate signed request binding against method, host, and path.
9. Evaluate scopes, roles, direct permissions, and requested operation.
10. Evaluate rate limits.
11. Record non-secret usage metadata and audit event.
12. Return authenticated principal.
```

Signed tokens should be very short lived. A Coinbase-like two-minute maximum is
a reasonable reference for high-risk operations. Longer windows should require
an explicit product and security decision.

Principal shape:

```text
ApiPrincipal
  owner_id
  identity_id
  key_id
  keyspace_id
  scopes
  roles
  allowed_vaults
  allowed_integrations
  allowed_environments
```

The principal can authorize a command or session. It must not directly fetch raw
credential material.

## Permissions And Roles

Use permissions for concrete capabilities:

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

Roles group permissions:

```text
Role
  name
  permissions[]
```

API keys may have roles and direct permissions. Direct permissions should be
used sparingly because they make audits harder.

An API key must not be able to create another API key with more authority than
the creator is allowed to delegate.

## Root And Management Keys

Separate high-authority root/management keys from ordinary API keys.

```text
Root key
  -> management API access
  -> can create, update, reroll, revoke, or verify keys according to permissions

Ordinary key
  -> product/API access
  -> scoped to specific operations and resources
```

Root keys should be:

- rare;
- environment-specific;
- permissioned;
- rotated periodically;
- forbidden from browser/client-side use;
- audited aggressively.
- signed-request keys by default once the signed-key mode exists.

## Rate Limits

Rate limits should attach to API keys, identities, owners, routes, or product
surfaces.

```text
RateLimitPolicy
  id
  subject_kind
  subject_id
  limit
  duration
  burst
  fallback_behavior
```

Verification should return rate-limit context so callers can set response
headers and logs without leaking secrets.

Apply rate limits to:

- public API calls;
- credential verification attempts;
- management API writes;
- gateway session creation;
- integration registration calls.

## Rerolling

Rerolling creates a new raw key while preserving the old key's policy.

```text
old key
  -> remains valid for grace period

new key
  -> fresh secret
  -> inherits metadata, permissions, roles, identity, and rate limits
  -> shown once

after grace
  -> old key revoked
```

This preserves audit continuity and avoids forcing users to recreate policy by
hand.

Reroll metadata:

```text
rerolled_from
rerolled_to
grace_period_expires_at
```

Expired or revoked keys should not be rerollable unless a recovery workflow
explicitly allows it.

Signed keys use public-key rotation instead of bearer rerolling:

```text
old public key
  -> remains accepted for grace period if policy allows

new public key
  -> inherits metadata, permissions, roles, identity, and rate limits
  -> private key stays with the caller

after grace
  -> old public key revoked
```

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

Recommended resource scopes:

```text
allowed_vaults
allowed_integrations
allowed_environments
allowed_runtime_services
allowed_hosts
```

Avoid broad workspace-wide API keys that can use every vault. Those create a
large blast radius.

## Security Requirements

- Never log raw API keys.
- Never put raw API keys in NATS messages, outbox messages, metrics, traces,
  URLs, workflow state, or durable saga payloads.
- Store only verifier digests for Trogonai-issued API keys.
- Store only public verification material for signed Trogonai API keys.
- Compare verifier digests in constant time.
- Verify signed-request tokens against method, host, path, timestamp, and
  replay policy before authorization.
- Return raw keys only once.
- Redact presented keys in validation errors and audit events.
- Record non-secret usage metadata such as key id, owner, operation, result,
  timestamp, and source address.
- Keep verifier pepper/key material out of the API-key table.
- Keep signed-key private material out of the API-key table.

## Observability

Emit metrics and logs for operations, not values:

- key verification count by result;
- permission denial count;
- rate-limit denial count;
- revoked/expired key attempts;
- reroll count;
- revoke count;
- create count;
- usage by key id or fingerprint if cardinality has been reviewed.

Audit event examples:

```text
api_key.created
api_key.verified
api_key.denied
api_key.rerolled
api_key.revoked
api_key.expired
api_key.permission_added
api_key.permission_removed
api_key.rate_limit_changed
api_key.public_key_added
api_key.public_key_revoked
api_key.signed_request_replayed
```

## API Surface

Suggested management endpoints:

```text
POST   /v1/api-keyspaces
GET    /v1/api-keyspaces
GET    /v1/api-keyspaces/{keyspace_id}
DELETE /v1/api-keyspaces/{keyspace_id}

POST   /v1/api-keys
GET    /v1/api-keys
GET    /v1/api-keys/{key_id}
POST   /v1/api-keys/{key_id}/reroll
POST   /v1/api-keys/{key_id}/revoke
PATCH  /v1/api-keys/{key_id}
POST   /v1/api-keys/{key_id}/public-keys
POST   /v1/api-keys/{key_id}/public-keys/{public_key_id}/revoke

POST   /v1/api-keys/verify
```

`GET` responses are metadata-only. `POST /v1/api-keys` and reroll responses may
return the raw key once.

`POST /v1/api-keys` should accept a `kind`:

```text
kind: bearer
  -> create verifier-backed raw key shown once

kind: signed
  -> register public key or generate one-time private key download
```

## Idempotency And Retry Safety

API-key management endpoints should be idempotent, especially create, reroll,
revoke, and public-key registration. A client retry must not create unlimited
dead keys.

The client provides an opaque idempotency key per logical user action. The
server scopes it after authentication:

```text
IdempotencyRecord
  owner_id
  workspace_id
  command_namespace
  target_resource_id
  idempotency_key
  request_fingerprint
  operation_id
  resource_id
  response_snapshot
  status
  created_by_actor_id
  created_at
  expires_at
```

Recommended command namespaces:

```text
api_key.create
api_key.reroll
api_key.revoke
api_key.public_key.add
api_key.public_key.revoke
api_key.patch_policy
```

Recommended uniqueness:

```text
unique(owner_id, command_namespace, idempotency_key)
```

For commands that target an existing key, include the target in the effective
scope:

```text
unique(owner_id, command_namespace, target_resource_id, idempotency_key)
```

Two different companies can use the same idempotency key safely because the
server includes the owner/workspace scope:

```text
company_a + api_key.create + abc123
company_b + api_key.create + abc123
```

Those are unrelated operations.

The contract is:

```text
same owner + same namespace + same key + same request fingerprint
  -> return the same operation/resource/status

same owner + same namespace + same key + different request fingerprint
  -> reject with idempotency conflict

different owner + same namespace + same key
  -> unrelated operation
```

The idempotency key is not an authorization token. Every retry must still
authenticate and authorize normally. By default, response replay should require
the same actor/client that created the idempotency record or an actor with
explicit permission to view the target key.

The response snapshot must be metadata-only:

```text
allowed in response_snapshot
  -> operation id
  -> API key id
  -> public prefix
  -> fingerprint
  -> status
  -> validation errors

forbidden in response_snapshot
  -> raw bearer key
  -> generated private key
  -> reusable token value
```

This creates an important product rule for one-time display values.

If a bearer API key or server-generated private key was created successfully but
the client lost the response, the backend must not redisplay the raw value from
the normal database. It should return metadata and require a reroll, unless the
product deliberately implements a short-lived one-time display escrow.

One-time display escrow, if used, must live behind the secret-store boundary
with a short TTL and explicit purge behavior:

```text
operation_id
  -> one-time display material in OpenBao or equivalent
  -> expires quickly
  -> deleted after successful display acknowledgement
  -> never copied into DB idempotency rows
```

Default recommendation: avoid one-time display escrow in the first version.
For lost create/reroll responses, show the key as created but unrecoverable and
ask the user to reroll.

Signed keys have a cleaner retry story when the client generates the key pair:

```text
client-generated signed key
  -> client keeps private key
  -> retry registers the same public key
  -> response replay is metadata-only
```

Use pending-operation limits to prevent accidental or malicious row growth:

```text
max pending api_key.create operations per owner
max pending api_key.reroll operations per key
max active root keys per owner
max active public keys per signed key
```

## Open Questions

- What exact key prefix format should Trogonai use?
- Should verifier digests use keyed hashing, Argon2id, HMAC-SHA-256 with pepper,
  or another verifier construction?
- Where should the verifier pepper live: OpenBao transit, OpenBao KV, KMS, or
  deployment secret?
- What are the first keyspaces?
- What is the root key bootstrap flow for self-hosted deployments?
- How much of RBAC belongs in the first milestone?
- Do we need identity-level shared rate limits immediately?
- What is the default reroll grace period?
- Which audit events are required for the first release?
- Which signed-key algorithm should be the first supported default: ES256 for
  Coinbase-style compatibility, Ed25519 for simpler modern signatures, or both?
- Should the first Coinbase-style signed flow use JWT request tokens only, with
  certificate-based client authentication left as a later extension?
- Should private keys be client-generated only, or should Trogonai offer a
  one-time server-generated download flow?
- What nonce or replay store should signed requests use at gateway scale?

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
