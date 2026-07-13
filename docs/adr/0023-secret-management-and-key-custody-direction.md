---
number: "0023"
slug: secret-management-and-key-custody-direction
status: proposed
date: 2026-07-11
---

# ADR 0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service

## Context

The platform is accumulating credentials it must hold on behalf of tenants,
and today it has no place to put them.

The first consumer is concrete. `trogon-gateway` ingests events from twelve
SaaS sources (eleven inbound webhook routes plus Discord's outbound WebSocket
gateway), and every integration carries a webhook signing secret, bot token,
or verification token. Those values currently resolve from environment
variables at config-load time through the `SecretInput` pattern, which
[ADR 0007](./0007-configuration-sources.md) sanctions as a bootstrap
mechanism, not a durable model: configuration files must not store secrets,
and env vars cannot express per-company, console-managed connections. The
second consumer is near-term direction rather than shipped code: the MCP
layer bridges MCP over NATS today, and connecting outbound to remote MCP
servers on a company's behalf will require presenting per-company
`Authorization` headers. Behind those two sit known future consumers: OAuth
refresh tokens for user-delegated outbound calls (the pattern the console
gateway of
[ADR 0018](./0018-connectrpc-gateway-for-browser-product-surfaces.md)
already sets by holding operator OAuth tokens server-side), and custody of
the platform's own JWT signing keys, for which `a2a-auth-callout` already
ships an unimplemented vault-backed signing-key source.

The product direction is per-company connections managed through the console:
a company is the tenant, and users may group their secrets into named vaults
inside their company. The platform will run as a hosted multi-tenant service
and also be self-hosted by third parties, so whatever this decision commits to
must be operable by people who are not us, under a real open source license.

"KMS" is an overloaded word, and this ADR deliberately splits it into
problems that must not share one interface:

- **Secret storage**: values an authorized workload must retrieve later
  (tokens, signing secrets, API credentials).
- **Cryptographic operations**: encryption, decryption, key wrapping,
  signing, where the key itself is never retrieved.
- **Password hashing**: intentionally one-way, owned by the application, and
  never a KMS concern.
- **Short-lived protocol tokens**: AAuth JWTs
  ([ADR 0017](./0017-aauth-agent-authentication.md)), CSRF tokens, and
  session state stay in their owning subsystems; central storage adds nothing
  to values designed to expire.

Prior art studied for this decision: GitLab's secrets manager built on
OpenBao (per-tenant namespaces, a control plane that provisions policies
without persisting tokens by using inline auth, direct reads by ephemeral CI
runners with short-lived JWTs), and the AWS KMS product family as the
managed-service baseline.

An earlier prototype already exercised the storage split this ADR wants,
embedded inside `trogon-gateway`: an event-sourced credential aggregate that
writes raw values to OpenBao KV v2 and everything else to NATS JetStream.
This ADR adopts that prototype's validated mechanics and supersedes its
embedded-in-gateway shape (Decision 4).

This ADR records direction. It is not approval to implement, and it names the
proofs required before adoption.

## Decision

### 1. OpenBao is the secret-store and key-management provider

OpenBao (the MPL-2.0, Linux Foundation governed fork of HashiCorp Vault) is
the single provider for both problems: KV v2 for secret storage now, Transit
for cryptographic operations when data encryption or signing custody arrives.

The selection rule it satisfies, and which any replacement must satisfy: the
deployed version carries an OSI-approved license, every required production
feature is available under that license, production use has no CPU, customer,
or service-provider restrictions, self-hosting needs no vendor license
server, and security-critical capabilities do not silently require a
proprietary edition. Source-available licenses (BUSL Vault, Cosmian) fail
this rule; that is why the ecosystem-compatible fork is the choice rather
than upstream Vault.

We commit to OpenBao as the one provider rather than maintaining a
multi-provider adapter matrix. Provider independence is preserved where it is
cheap and load-bearing, at the application boundary; it is not pursued as a
feature in itself.

### 2. Two typed boundaries, not one generic interface

Business code sees application concepts only, through two separate ports:

- **SecretStore**: store and resolve retrievable credentials. Callers hold a
  `SecretRef`, an opaque platform handle that resolves internally to
  tenant, vault, and key. It is never an OpenBao path.
- **KeyManagement**: encrypt, decrypt, wrap, rewrap, sign, verify, against a
  `KeyRef` with an explicit key purpose. Keys are never retrievable.

OpenBao paths, tokens, policy names, and Transit ciphertext version markers
must not cross either boundary, appear in wire contracts, or land in business
entities. A caller that can parse a provider artifact is a caller that will
eventually depend on it.

Material placement follows from the split: provider OAuth tokens, webhook
signing secrets, bot tokens, and API credentials live in the SecretStore;
KEKs and non-exportable signing keys live behind KeyManagement; platform-
issued API keys are stored as one-way verifiers, never as retrievable values
(the HMAC-digest approach of the deprecated `ApiKeyRegistry` is the shape to
keep, whatever store it lands in); user passwords are password hashes, full
stop; integration metadata, ownership, and status are
product state in the application database, with only the credential material
itself behind the boundary.

### 3. Workloads never talk to OpenBao: the secrets service is the only client

A platform secrets service fronts OpenBao. It is the only process holding an
OpenBao client or credential. `trogon-gateway`, the MCP bridging layer, and
every future consumer hold `SecretRef`s and resolve them on demand.

The wire shape is deliberate:

- Resolution is NATS **core request/reply**, authenticated by the
  tenant-scoped NATS user JWTs `a2a-auth-callout` already mints. No new
  credential type, no new network path, and the NATS account (the tenant
  unit) scopes who can even address a company's resolve subjects.
- Secret values never traverse JetStream. Durable, replicated, replayable
  streams are where plaintext goes to be forgotten about; a value in a stream
  is a secret at rest outside the store. JetStream carries the metadata
  around secrets instead: rotation, revocation, and cache-invalidation
  events, and audit correlation.
- Tenant resolution is server-side, always derived from the authenticated
  connection identity, never from a caller-supplied field. Vault-level
  granularity inside a company is authorization policy (the SpiceDB and CEL
  tiers the A2A gateway already runs), not subject naming.

Two alternatives were considered and rejected:

- **Direct reads** (each workload authenticates to OpenBao itself, the
  GitLab-runner model). GitLab's property comes from ephemeral, single-tenant,
  per-job runners holding narrowly scoped short-lived JWTs. Our consumers are
  the opposite shape: long-lived, multi-tenant, and the most internet-exposed
  processes on the platform. `trogon-gateway` parses untrusted payloads from
  eleven webhook providers; the outbound MCP path, once built, talks to
  arbitrary remote MCP servers. Standing OpenBao credentials in those
  processes turn any parsing
  or SSRF bug into a pivot toward tenant secret trees, duplicate OpenBao auth
  wiring per language, and fork the architecture permanently the day a
  customer-run gateway appears.
- **Control-plane injection** (the control plane resolves values and delivers
  them with dynamic configuration). Values riding config distribution either
  land in JetStream (plaintext at rest, replicated, in backups) or lose
  delivery guarantees; rotation requires reliable re-push to every replica;
  and the control plane becomes the mandatory plaintext custodian for every
  tenant, which structurally forecloses customer-held keys. Injection remains
  acceptable only for genuinely non-secret dynamic config.

The GitLab property worth keeping survives in a different place: the secrets
service mints short-lived, narrowly scoped OpenBao credentials per request
internally, rather than holding one standing broad token. That internal
choice carries most of the design's security value.

Minting requires the service to first authenticate itself, so the bootstrap
path is part of this decision rather than an implementation detail:

- **Bootstrap identity comes from the deployment platform, not from
  configuration.** The service logs in through an OpenBao auth method bound
  to an identity the deployment attests: service-account JWT auth where the
  orchestrator provides one, AppRole or TLS certificate auth as the
  self-hosted baseline. No standing OpenBao token lives in a config file or
  environment variable; the ADR 0007 rule applies to the secrets service
  itself.
- **The bootstrap policy is minting-only.** Login yields a short-TTL token
  whose ACL permits creating the per-request child credentials under the
  platform's own mounts, renewing itself, and nothing else: no system
  administration, no seal or audit configuration, no direct secret reads
  outside the derived path prefixes.
- **Expiry is the rotation trigger.** The bootstrap token carries a short
  TTL and a bounded max TTL, so routine rotation is re-login. Redeployment
  rotates it implicitly, and suspected compromise is handled by revoking the
  auth role's issued tokens, which severs every instance at once.
- **Bootstrap failure fails closed.** An instance that cannot authenticate,
  at startup or when renewal lapses, does not report ready, does not join
  the resolve queue group, and retries with backoff. There is no degraded
  mode that serves values past the cache bounds of Decision 6.
- **Break-glass is OpenBao's own ceremony, out of band.** Self-hosted
  recovery is the documented OpenBao unseal and root-token-generation
  procedure (quorum-held key shares, root token revoked after use), executed
  by operators directly against OpenBao. The platform never automates or
  exposes a root-credential path.

### 4. OpenBao provides primitives, not a schema: the service owns the data model

OpenBao imposes no application schema. Its entire structure is secret engine
mounts, hierarchical paths, versioned KV v2 JSON values, per-secret custom
metadata, path-scoped ACL policies, and namespaces. It has no concept of an
integration, an owner, a credential kind, a lifecycle state, a delivery
policy, or a user-visible vault grouping. Every domain concept is modeled by
the platform, stored in the platform's own state, and mapped onto OpenBao
primitives by exactly one component.

Consequences of that fact:

- **The secrets service owns the path convention.** OpenBao paths are derived
  deterministically from validated, immutable domain identifiers by the
  service's OpenBao adapter, and nowhere else. No caller ever supplies a
  path. This is the same lesson GitLab's `path_builder` encodes: name paths
  by immutable IDs so renames never move data.
- **Reuse OpenBao mechanics instead of duplicating them.** The KV v2 version
  counter is the credential version; credential status derives from the
  key-level `current_version` pointer plus the per-version `destroyed` and
  `deletion_time` fields rather than the platform tracking it in parallel;
  custom metadata carries only non-secret reconciliation breadcrumbs.
- **The write model is already validated.** A gateway-embedded prototype
  built it end to end: an event-sourced credential aggregate on JetStream
  whose events and snapshots carry refs, fingerprints, and reason enums but
  never values; a write saga (pending event, OpenBao write, activation with
  metadata only); and a recovery worker that reconciles stuck states by
  reading OpenBao metadata, never values. There is no distributed
  transaction between the platform's state and OpenBao, only the saga and
  reconciliation. Adopting this ADR means extracting that machinery from
  `trogon-gateway` (which in the prototype is itself the OpenBao client)
  into the secrets service, not rebuilding it.
- **The resolve surface needs an operation verb, not only a value verb.**
  The prior work surfaces compute-in-place needs (signing with an issuer key
  that must never leave custody, verifier peppers). Those are KeyManagement
  operations arriving early; they belong behind the same service so signing
  consumers never bypass the single-client rule by calling Transit directly.

### 5. Tenancy and isolation

The company is the tenant, aligned with the NATS account that already scopes
connection admission. Vaults are user-visible logical groupings within a
company, enforced by policy, not by cryptographic separation.

Hard isolation options (per-tenant OpenBao namespaces, per-tenant seals,
customer-supplied keys) are explicitly not committed and explicitly not
foreclosed: because no consumer ever addresses OpenBao, adopting any of them
later is an internal routing change in the secrets service, invisible to
callers.

### 6. Operational rules

- **Fail closed.** A secret that cannot be resolved is an operation that does
  not happen. There is no plaintext fallback, and no secret in a config file
  ever again once a consumer is migrated.
- **Bounded caches only**, with event-driven invalidation from the rotation
  events as the primary mechanism and TTL as backstop. Invalidation is
  fan-out, not work-sharing: resolve traffic load-balances across the queue
  group, but rotation and revocation events must reach every secrets-service
  instance, each replica reading the JetStream metadata stream through its
  own consumer, because queue-group delivery would evict one cache and leave
  the others serving a revoked credential. The cache TTL is therefore the
  maximum stale window after a missed event, and it is bounded in minutes,
  not hours. Consumers hold values for the duration of an in-flight
  operation, not as ambient state.
- **Dual audit, built once.** The service records the business context
  (caller, tenant, vault, ref, purpose, outcome) with a correlation ID
  threaded into the OpenBao request, so the provider's own audit log joins
  back to who asked and why. Neither log substitutes for the other.
- **Key state is explicit** (active, decrypt-only, disabled, pending
  destruction, destroyed), destruction is scheduled and reviewable rather
  than an immediate API operation, and a compromised KEK means re-encryption
  with new DEKs, not just rewrapping.

### 7. Evolution path

- Webhook signature verification in `trogon-gateway` can later become an
  operation rather than a value: the service verifies the HMAC via Transit
  and returns a verdict, and the signing secret never leaves OpenBao at all.
- The `a2a-auth-callout` vault-backed signing-key source becomes a real
  KeyManagement caller with no new wire protocol.
- Customer-run gateways against the hosted control plane need only the NATS
  identity they already require to participate; OpenBao is never
  network-exposed to infrastructure the platform does not operate.
- Self-hosted parity is running the same secrets service against your own
  OpenBao; consumer binaries are identical in hosted and self-hosted
  deployments.

## Consequences

- The repository gains a new workload class, the secrets service. It sits on
  the webhook and outbound-MCP hot paths, so it runs horizontally scaled
  behind a NATS queue group, and it is the platform's highest-value
  compromise target, which is why the per-request scoped-token rule and
  anomaly detection on resolve subjects are structural rather than optional.
- Per-company connections require dynamic gateway configuration, which is its
  own decision; this ADR only fixes the shape of the credential half
  (references in config, values behind the service).
- The gateway-embedded credential prototype is superseded in shape but not
  in substance: its OpenBao adapter, credential aggregate, write saga, and
  recovery worker are the intended starting point for the secrets service's
  write model, relocated out of `trogon-gateway`.
- Self-hosters inherit OpenBao operations (HA, unsealing, snapshots). A
  dev-mode story (single node, static seal, container) is required so the
  full operational burden is a production concern, not the price of trying
  the framework.
- OpenBao is not an HSM. Key material exists in server process memory, and
  there is no FIPS-validated build today. If a certified hardware boundary
  ever becomes a requirement, that is a new decision, not a configuration of
  this one.
- Adoption is gated on demonstrated proofs, not enthusiasm: HA failover under
  live traffic, cold restore and unseal from documented backups, denial of
  cross-tenant and cross-purpose access, rotation with old ciphertext still
  readable, fail-closed startup when the service cannot authenticate to
  OpenBao, a rotation event observed to evict every replica's cache, correct
  behavior when audit sinks fail, and acceptable latency on the resolve
  path.

## References

- [ADR 0007: Configuration Sources](./0007-configuration-sources.md)
- [ADR 0017: AAuth Agent Authentication over a Trogon NATS PoP Binding](./0017-aauth-agent-authentication.md)
- [ADR 0018: ConnectRPC Gateway for Browser Product Surfaces](./0018-connectrpc-gateway-for-browser-product-surfaces.md)
- [OpenBao documentation](https://openbao.org/docs/)
- [OpenBao Transit engine](https://openbao.org/docs/secrets/transit/)
- [OpenBao KV v2](https://openbao.org/docs/secrets/kv/)
- [GitLab Secret Manager architecture blueprint](https://handbook.gitlab.com/handbook/engineering/architecture/design-documents/secret_manager/)
- [NIST SP 800-57 Part 1: key lifecycle guidance](https://csrc.nist.gov/pubs/sp/800/57/pt1/r5/final)
