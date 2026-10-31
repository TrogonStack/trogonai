---
number: "0083"
slug: sealed-content-envelope
status: accepted
date: 2026-10-30
---

# ADR#0083: Credential Material, Claim-Check Blobs, and Sensitive Tenant Streams Under a Customer Managed Key Are Sealed in an Owned AEAD Envelope, and Nothing Else Is

## Context

[ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md) puts TLS
on every NATS connection and encrypts every JetStream store, but neither stops a
reader the server admits or keeps payloads out of logs. The cipher "protects
disks and their copies, not one tenant's data from another's; the account
boundary does that" (Decision 6), and that boundary stops at `PLATFORM`, whose
shared workloads read every tenant's streams.

Secret values cross in plaintext. The OpenBao adapter holds the response body in
a `String` and the value in a `SecretString`
(`rsworkspace/crates/platform/trogon-gateway/src/secret_store/openbao_secret_store.rs:135-138`,
`:316-317`), an `Arc<str>` never zeroized
(`rsworkspace/crates/platform/trogon-std/src/secret_string.rs:5-6`, `:13-18`),
and `trogon-nats` logs every response payload at debug level
(`rsworkspace/crates/platform/trogon-nats/src/messaging.rs:82-83`). Credential
records hold no values but inventory which tenant holds which credential
(`trogon-gateway/src/credential/constants.rs:3-6`), and the idempotency
fingerprint (`credential_management/idempotency.rs:428-432`) is an HMAC keyed by
the admin token over the request, secret value included
(`credential_management.rs:655-666`, `:2109-2116`), so a holder of that token
who can read the bucket confirms a guessed secret offline.

Claim-check blobs carry payloads too large for a message
(`trogon-nats/src/jetstream/claim_check.rs:235-291`). Every tenant's blobs share
the `trogon-claims` bucket (`trogon-nats/src/constants.rs:31`,
`jetstream/object_store.rs:178-187`), whose chunks sit on
`$O.<bucket>.C.<nuid>`, so no subject permission confines a reader to one
tenant. Objects are named by the SHA-256 of their payload
(`claim_check.rs:224-233`), a confirmation oracle that is also logged (`:272`);
the resolver fetches whatever `Trogon-Claim-Key` names (`:56-71`); event-sourced
blobs never expire (`jetstream/claim_retention.rs:26-29`).

Tenant streams store payloads as plaintext `content`
(`rsworkspace/crates/decider/trogon-decider-runtime/src/event/mod.rs:26-35`)
under `Nats-Msg-Id` and `Trogon-Event-Type` headers
(`trogon-decider-nats/src/stream_store.rs:443-457`); the
[ADR#0057](./0057-decider-command-nats-binding.md) reply returns "the events it
appended" (`proto/trogonai/decider/v1/decided_event.proto:12-28`); session
artifacts are named by the digest of their bytes
(`proto/trogonai/session/sessions/v1alpha1/artifact.proto:23`, `:73`). The veto
a [customer managed key](../glossary/customer-managed-key) gives its owner
([ADR#0033](./0033-key-custody-product-model.md) Decision 1) binds to nothing.

The workspace declares no `aes-gcm`, `chacha20poly1305`, `zeroize`, `secrecy` or
`hpke` dependency (`rsworkspace/Cargo.toml`). It builds nkeys with xkeys
(`Cargo.toml:186`), and under [ADR#0015](./0015-rust-tls-library.md) rustls runs
on aws-lc-rs 1.18.1 (`:187`, `trogon-std/src/tls.rs:24-28`), which has
AES-256-GCM with random nonces, HKDF and HMAC but no HPKE; zeroize 1.8.2 is
locked transitively. The dev compose runs OpenBao 2.6.2
(`devops/docker/compose/compose.yml:101`), whose bootstrap enables Transit with
upsert disabled and creates one key, `local-dev-kek`, that every caller shares
(`devops/docker/compose/services/openbao/bootstrap.sh:16-17`, `:51-62`).

[ADR#0030](./0030-customer-controlled-key-backend-routing.md) leaves the payload
cipher, nonce construction and envelope to a decision that "must exist before
implementation", and two-party control to "a separate capability and decision",
as [ADR#0033](./0033-key-custody-product-model.md) Decision 1 repeats. [ADR#0033](./0033-key-custody-product-model.md) rejects envelope-encrypting all
platform state and keeps infrastructure encryption out of business key
management (Decision 6). [ADR#0035](./0035-session-store-decider-aggregate.md)
Decision 7 defers erasure-grade deletion.

## Decision

Sealed means stored in `SealedContent`, which replaces the payload inside an
[event envelope](../glossary/event-envelope); xkey-sealed means sealed to a NATS
xkey, as in
[ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
Decision 5. JetStream's and OpenBao's sealing keep their own meanings.

### 1. Sealing is a layer of its own, applied to named classes and nothing else

| Layer | Defeats | Decided by |
| --- | --- | --- |
| TLS | A network observer or tamperer | [ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md) Decision 1 |
| Server cipher | A holder of a disk, snapshot or file backup | [ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md) Decision 6 |
| Sealed content | An admitted reader without a lease, logs, traces and every copy | This decision |

Sealing also binds a customer managed key's veto to every sealed record, and
destroying a key erases what it sealed. It does not stop an attacker who
controls the NATS server, which can request a lease as any workload, since a
requester is known by its subject ([ADR#0082](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md) Decision 1), or substitute an xkey;
binding an xkey to an identity the server cannot forge is left open.

- **Secret values in transit.** A reply carrying a secret value or a lease is
  xkey-sealed to the requester's ephemeral xkey, with the secrets service
  authenticated as sender, and a submitted value to the service's xkey, for
  OpenBao and [ADR#0079](./0079-external-secret-sources.md) sources alike;
  [ADR#0023](./0023-secret-management-and-key-custody-direction.md) Decision 3
  still keeps values out of JetStream.
- **Credential records, for every tenant.** Events, snapshots and idempotency
  records, which [ADR#0074](./0074-credential-platform-extraction-boundary.md)
  Decision 1 moves into the secrets service, are sealed under leases it issues
  itself; the gateway's runtime projection becomes an opener and fails closed.
- **Sensitive streams of customer managed tenants.** Platform code declares a
  stream sensitive, never a tenant setting; this decision declares the [ADR#0035](./0035-session-store-decider-aggregate.md)
  session streams. Bytes such a stream references are sealed too (Decision 6).
- **Claim-check blobs, for every tenant**, even when the body is already sealed.

State derived from sealed records is sealed under its own store's series, and a
store with no layout here, such as a relational projection, holds none. API key
verifiers, keyed digests that must not wait on OpenBao
([ADR#0075](./0075-api-key-format-and-verifier-construction.md) Decision 3),
cold-start state under [ADR#0033](./0033-key-custody-product-model.md) Decision 5, this decision's buckets included,
and other platform state stay unsealed, and OpenBao KV v2 values stay outside
the veto, since [ADR#0033](./0033-key-custody-product-model.md) Decision 7 keeps the secret store out of the pluggable
part. Subjects, routing, trace, sequence and batch headers, `Nats-Msg-Id`,
`Trogon-Event-Type`, ids and claim headers stay plaintext so routing, replay and
tracing work, and no sensitive value goes in a header, `Trogon-Header-` included
(`trogon-decider-nats/src/constants.rs:4`).

### 2. `SealedContent` is a protobuf envelope that names its keys and AAD layout, never a provider

It lives in `proto/trogonai/platform/sealing/v1/`, one type per file under
[ADR#0009](./0009-protocol-buffers-wire-contracts.md). In sketch:

```proto
message SealedContent {
  SealingAlgorithm algorithm = 1;
  DataKeyRef key_ref = 2;
  bytes nonce = 3;
  bytes ciphertext = 4; // GCM tag appended
  repeated DekWrap wrapped_deks = 5;
  AadDescriptor aad_descriptor = 6;
}
message DataKeyRef { DataKeySeriesId series_id = 1; uint32 generation = 2; }
message DekWrap {
  KeyRef kek_ref = 1; // the opaque KeyRef of key backend routing
  WrappingAlgorithm wrapping_algorithm = 2;
  bytes wrapped_dek = 3; // a key backend routing WrappedDek
}
message AadDescriptor { AadLayout layout = 1; }
```

`SealingAlgorithm` starts with `SEALING_ALGORITHM_AES256_GCM_HKDF_SHA256`,
`WrappingAlgorithm` with `WRAPPING_ALGORITHM_ENVELOPE_WRAPPING_V1`, and
`AadLayout` names the layouts of Decision 3, each after an `_UNSPECIFIED` zero;
`DataKeySeriesId` and `KeyRef` wrap a string. Singular fields are
`LEGACY_REQUIRED`, enums `defined_only` with `not_in: [0]`, `nonce` is
`bytes.len = 12`, `generation` `uint32.gt = 0` and `wrapped_deks`
`repeated.min_items = 1`. Openers refuse unknown values and have no fallback
suite; evolution adds values, never reusing one. Each `DekWrap` persists a
`KeyRef`, the binding version as `wrapping_algorithm` and a `WrappedDek`
together ([ADR#0030](./0030-customer-controlled-key-backend-routing.md) Decision 2), and only the adapter parses `wrapped_dek`, which
holds a share, a port `Dek` in its own right; the data key is derived, never
wrapped whole. No backend, provider key, version or locator appears ([ADR#0030](./0030-customer-controlled-key-backend-routing.md)
Decision 4, [ADR#0033](./0033-key-custody-product-model.md) Decision 4). `KeyRef` keeps [ADR#0030](./0030-customer-controlled-key-backend-routing.md)'s name, as `SecretRef`
does, over the `_id` that
[ADR#0068](./0068-reference-fields-say-they-are-references.md) and
[ADR#0040](./0040-contract-field-vocabulary.md) give opaque identifiers. Wraps
follow tenant-state order, platform share first, and every `kek_ref` is
authorized against tenant state before any unwrap ([ADR#0030](./0030-customer-controlled-key-backend-routing.md) Decision 7).

Sealing needs generated Rust types, so `sealing/v1` and the `key_custody`
packages it imports are promoted into code generation, which brings
`buf/validate/validate.proto` into the generated crate. This promotion is the
moment [ADR#0064](./0064-schema-constraints-as-documentation.md) Decision 3
reserves for deciding enforcement, and the decision is that no generated
validator runs. The constraints are enforced where a message crosses into
code: `Opener`, the lease client and the lease service parse every field into
a value object, refuse what fails to parse, and never act on a raw generated
message. A validator would check the same rules a second time and would still
not replace the parsing, since the parsing is what the code acts on.

### 3. AES-256-GCM through aws-lc-rs, a key derived from shares, and a context the opener supplies

Sealing uses aws-lc-rs `aead::RandomizedNonceKey` over `AES_256_GCM`, a random
96-bit nonce and a 128-bit tag per seal, and HKDF-SHA256 ([RFC
5869](https://www.rfc-editor.org/rfc/rfc5869)), all from the provider [ADR#0015](./0015-rust-tls-library.md)
installs, so aws-lc-rs and zeroize become direct dependencies at their locked
versions and no new cryptographic library arrives.

```text
lp(x)      = u32_be(len(x)) || x
prk        = HKDF-Extract(salt = "", ikm = share_1 || ... || share_n)
dek        = HKDF-Expand(prk, lp("trogon/sealing/dek/v1") || u32_be(algorithm)
                              || lp(series_id) || u32_be(generation), 32)
naming_key = HKDF-Expand(prk, lp("trogon/sealing/naming/v1")
                              || lp(series_id) || u32_be(generation), 32)
aad        = lp(label) || lp(tenant) || lp(field_1) || ... || lp(field_k)
```

Shares are 32 bytes in `wrapped_deks` order, `algorithm` is the enum number,
`tenant` the account public key, and strings their exact UTF-8 bytes. KEK
references stay out of the derivation, so a rewrap keeps the data key.

| Layout | Label | Fields after the tenant |
| --- | --- | --- |
| Stream event | `trogon/sealing/stream-event/v1` | stream name, stored subject, `Nats-Msg-Id`, `Trogon-Event-Type` |
| KV record | `trogon/sealing/kv-record/v1` | `KV_<bucket>`, key, full name of the value's message |
| Claim blob | `trogon/sealing/claim-blob/v1` | `OBJ_<bucket>`, object name |
| Object | `trogon/sealing/object/v1` | `OBJ_<bucket>`, object name from the event's `StorageLocator`, full name of the message holding it |

The label versions the layout and the message name the payload schema. An opener
builds the context from its configuration and the message it holds, never from
the envelope, taking the tenant from the account public key that leads a subject
in `PLATFORM` ([ADR#0081](./0081-nats-tls-by-default-and-operator-mode-admission.md) Decision 3), a KV key after any version prefix, or an
object name through its subject. It refuses an envelope whose layout is not its
store's, whose `series_id` is not `<tenant>.<store>`, or whose wraps are out of
order. Writers on a sealing-required stream always set both headers, so a record
with either altered or missing fails. A backup, restore or mirror that keeps
subjects opens under the original stream's context, which a mirror's
configuration names; a copy that changes subjects is a rebuild.

A generation seals at most 2^30 units, a seal of `len` bytes costing
`max(1, ceil(len / 2^20))`. That keeps invocations under the 2^32 that [NIST SP
800-38D](https://csrc.nist.gov/pubs/sp/800/38/d/final) section 8.3 allows for
random nonces, with a nonce collision at most 2^60 / 2^97 = 2^-37 likely, and
volume under 2^46 blocks, bounding the distinguishing advantage at 2^92 /
2^129 = 2^-37, about 2^-36 together. A single seal is at most 2^12 units
(4 GiB), so one invocation stays under 2^28 blocks, far inside the 2^32 - 2
blocks the same section allows per invocation; a larger payload is a claim
blob split across seals, never one. A generation also ends after 30 days, so
the unit cap binds first only above about 414 units a second.

GCM does not commit to its key, and nothing here needs it to: `key_ref` and the
context fix one data key and an opener never tries another, while invisible
salamanders ([Dodis et al.](https://eprint.iacr.org/2019/016)) and partitioning
oracles ([Len et al.](https://eprint.iacr.org/2020/1491)) need a receiver that
does, so a malicious sealer gains nothing beyond its lease. A suite that tries
candidate keys or opens from a subset of shares must commit.

### 4. Data keys come in generations, reach workloads only as leases, and need every share their tier requires

The secrets service is the [ADR#0030](./0030-customer-controlled-key-backend-routing.md) encryption caller for sealing, and no other
process holds a share. A series is `<tenant>.<store>`, the store being the
stream name, `KV_<bucket>` or `OBJ_<bucket>`, with generations from 1, and the
`SEALING_DATA_KEYS` bucket holds each series' current `DataKeyRef`, wraps,
creation time, reserved units, sealing state and cutover. Leases reserve units
by compare-and-set, sized by the requester, capped by the service and never
returned; either cap starts a new generation, and when replicas race, the
compare-and-set winner stands and the others zeroize theirs.

Each tenant gets its own sealing Transit key,
`sealing-<tenant account public key>` behind the adapter-private locator,
created as `aes256-gcm96` with upsert disabled, as `bootstrap.sh:55-62` does.
The platform share comes from Transit `datakey/plaintext` on it, the typed
optional capability of [ADR#0030](./0030-customer-controlled-key-backend-routing.md) Decision 2, or else is 32 CSPRNG bytes passed to
`wrap_dek`; the customer share is 32 CSPRNG bytes wrapped under the customer
managed `KeyRef`. Each share is wrapped under an [ADR#0030](./0030-customer-controlled-key-backend-routing.md) `AuthenticatedBinding`
of the tenant, the series and generation as resource, the sealing purpose and
schema version 1, passed to Transit as `associated_data`.

A lease, sent over core request/reply and xkey-sealed to the requester's
ephemeral xkey, carries the data key, a sealer's naming key, the `DataKeyRef`
and wraps, reserved units, deadline, the issuer's last applied invalidation
revision and the cutover, never a share. The requester authenticates the
service's public xkey, boot material under [ADR#0033](./0033-key-custody-product-model.md) Decision 5, and checks the
reply's series and generation. A request to open carries the envelope's
`DataKeyRef` and wraps, since the registry keeps only the current generation.
Tenant and workload come from the subject, grants
are per workload, tenant, store, and seal or open, and holders are `PLATFORM`
workloads; one in a tenant account would need an export, left open.

Sealing has no shared managed key: [ADR#0033](./0033-key-custody-product-model.md) Decision 3 makes dedicated keys
platform policy, free on Transit, so each tenant's platform share is wrapped
under its own key, for per-tenant rotation, audit and destruction, and a cloud
KMS's per-key bill is accepted. A customer managed tenant also needs the
customer share, with no fallback, and no wrap is optional in any tier. Splitting
the key is the two-party control [ADR#0030](./0030-customer-controlled-key-backend-routing.md) and [ADR#0033](./0033-key-custody-product-model.md) leave to a separate
decision, taken here for sealing only: the platform erases without the customer,
the veto is cryptographic, and every tier runs one code path, while each call
still routes to exactly one registration ([ADR#0033](./0033-key-custody-product-model.md) Decision 5).

Tier and KEK changes apply from the next generation, and KV records rewrap or
re-seal by compare-and-set ([ADR#0030](./0030-customer-controlled-key-backend-routing.md) Decision 4). Stream records cannot be
rewritten, so moving them is a rebuild into a new
[ADR#0059](./0059-session-stream-incarnation-fencing.md) incarnation while old
wraps still unwrap, and because an incarnation belongs to the physical stream
([ADR#0059](./0059-session-stream-incarnation-fencing.md) Decision 2), it moves every tenant on it. A compromise means
re-sealing under new generations, never rewrapping alone ([ADR#0023](./0023-secret-management-and-key-custody-direction.md) Decision 6),
and `min_decryption_version` never rises while an older wrap exists.

Before the lease service answers the first request for a series, and again
whenever that series' registration changes, it reconciles the registry entry
against the backends: the sealing key exists, has the type and purpose the
series needs, and the oldest live wrap still unwraps. A series that fails the
check serves no leases; the refusal and the audit record name the key's locator
and never its material, and every other series keeps serving. No caller falls
back to plaintext, and a key destroyed under Decision 8 fails this check by
design.

### 5. Each series crosses from plaintext to sealed once, at a recorded cutover

A store created sealing-required has its cutover at its start; any other crosses
once, one way, at a cutover the registry records. Sealed records are marked at
the [ADR#0009](./0009-protocol-buffers-wire-contracts.md) boundary: a stream record by a `Trogon-Sealed` header naming
`trogonai.platform.sealing.v1.SealedContent`, a KV record by its key's version
prefix, such as `v2.` where plaintext keys use `v1.`, and a claim by
`Trogon-Claim-Check: v2`. The service starts a crossing by setting the series to
sealing, and every writer of a sealable store asks for a lease at least once per
lease lifetime, getting no key while its series is plaintext. After two
lifetimes the service records the cutover: for a stream, the sequence after the
last unmarked record on the tenant's subjects; for a bucket, once the owner has
re-sealed or deleted the tenant's unmarked records. Before the cutover a reader
decodes by the marker; from it on every record must open, and any failure, an
unmarked record included, is an `IntegrityFailure`, so no writer can downgrade a
series. Tenants in a shared stream cross separately, and before any cutover the
[ADR#0057](./0057-decider-command-nats-binding.md) reply changes to return events as stored or by id.

Credential records cross at the [ADR#0074](./0074-credential-platform-extraction-boundary.md) Q4 re-key, whose streams start
sealing-required with the tenant token ahead of the project id, because the
secrets service is a `PLATFORM` decider serving several tenants and builds its
[ADR#0027](./0027-decider-multi-tenancy-primitive.md) `SubjectScope` from it;
idempotency records expire within a day
(`credential_management/constants.rs:11`). Elsewhere, history before a cutover
stays outside the veto and erasure until a rebuild re-seals it.

### 6. A claim-check blob is always sealed and named by a keyed digest

The publisher seals every blob with the claim-blob layout under
`<tenant>.OBJ_<bucket>`, taking the tenant from the subject as an opener does,
or using the `PLATFORM` account's own series when a subject names no tenant.
With a message id the object is named
`{subject}/{hex(sha256(Nats-Msg-Id))}/{hex(HMAC-SHA256(naming_key, lp(Nats-Msg-Id) || payload))}`,
and without one `{subject}/{uuid v7}`. Retries within a generation still find
their object (`claim_check.rs:276-279`); the name is no oracle and shows no
equality between messages, which defuses the key log at `:272`; a retry across
generations orphans an object, as a failed publish already does.

The resolver opens `v2` claims with the claim-blob layout and, before fetching
any claim, `v1` included, requires `Trogon-Claim-Key` to start with
`{subject}/{hex(sha256(Nats-Msg-Id))}/`, or `{subject}/` without a message id;
sealing the whole buffer matches the whole-object read at `:71`. On a stream
that is not sealing-required the marker decides, conceding nothing to a writer
that could publish plaintext anyway.

Referenced bytes, such as session artifacts, are sealed with the object layout
under their bucket's series, named by a keyed digest and located by a
`StorageLocator` that the session `StoredArtifact` gains. For customer managed
tenants this replaces [ADR#0035](./0035-session-store-decider-aggregate.md) Decision 3's plaintext-digest naming, and the
plaintext digest moves inside the sealed event to check the opened bytes.

### 7. Data keys are cached for at most sixty seconds and evicted on invalidation

Every sealer and opener, the secrets service included, keeps one cache keyed by
`DataKeyRef` and a digest of the wraps. A lease lives a random 54 to 60 seconds,
capped by the issuer's entry, which lives at most 60 seconds from its provider
unwrap; an issuer with under 54 seconds left unwraps again. A holder's deadline
starts when it sends the request, runs on a monotonic clock and never extends,
so once provider enforcement is effective a data key opens content for at most
sixty seconds more. For sealing data keys only, this amends [ADR#0030](./0030-customer-controlled-key-backend-routing.md) Decision
2's rule against reusable caches, Decision 6's no-retention sentence and
guarantee, Decision 7's cache bullet, and [ADR#0023](./0023-secret-management-and-key-custody-direction.md) Decision 6's "Consumers hold
values for the duration of an in-flight operation"; the no-fallback rule stands.

`SEALING_KEY_INVALIDATIONS`, a KV bucket in `PLATFORM` keyed by series id with
history 1 and a one-hour maximum age, holds a versioned protobuf naming the
reason. To disable or destroy a key the service disables it at the provider,
evicts its own cache, then writes one entry per affected series, as on a
`KeyUseDenied` from a mandatory backend. Holders evict on any entry, even one
they cannot decode, start with `watch_with_history`, resume with
`watch_from_revision`, and refuse a lease whose issuer revision predates the
latest invalidation they applied for its series. A holder whose watch is down
takes no new lease and serves what it holds only to the deadlines, so an hour of
history is enough.

The [ADR#0049](./0049-revocation-latency-target.md) target, p99 at or under five
seconds, applies per holder from an entry's stored time to eviction, with the
deadline as backstop. Sixty seconds matches the minute [ADR#0030](./0030-customer-controlled-key-backend-routing.md) Decision 6
records for a typical Google Cloud KMS disablement, sits far inside [ADR#0049](./0049-revocation-latency-target.md)'s
330-second backstop, and holds provider load to one unwrap per issuer replica
per active generation per minute; unwrapping per event would put provider
latency and quota on every replay and fan-out.

Data keys, naming keys and shares live in zeroizing buffers, and entries zeroize
on eviction and drop. `SecretString` becomes an `Arc` over a zeroizing
`Box<str>`, copied from an owned `String` that is then zeroized; the adapter's
body and value move into zeroizing buffers, and the debug payload log goes.
Erasure from memory stays best-effort ([ADR#0030](./0030-customer-controlled-key-backend-routing.md) Decision 2).

### 8. Destroying a tenant's sealing key erases its sealed content

Nothing starts before the business audit record of [ADR#0023](./0023-secret-management-and-key-custody-direction.md) Decision 6's dual
audit is written. The key is then soft-deleted, Pending destruction in [key
states](../architecture/key-states.md): OpenBao 2.6.2 refuses every use, the
service refuses the tenant's leases, and caches are evicted and invalidations
written (Decision 7). When the review window, whose length key states leaves
open, passes without a restore, `deletion_allowed` is set in the key's `config`
and the key deleted, which is Destroyed, and the completion audit record is
written; upsert stays disabled, so "OpenBao deletion cannot trigger encrypt-path
key recreation" ([ADR#0030](./0030-customer-controlled-key-backend-routing.md) Decision 7).

Every generation and copy of the tenant's sealed content becomes unreadable,
retired incarnations, mirrors and backups included, once no OpenBao backup still
holds the key. For a customer managed tenant either party can shred alone.
Losing OpenBao for good, as losing its seal key would
([ADR#0052](./0052-cloud-kms-production-seal.md)), loses sealed content too.
Shredding is tenant-granular: it un-defers [ADR#0035](./0035-session-store-decider-aggregate.md) Decision 7 at that
granularity, and per-session erasure stays deferred, since a key per session
multiplies registry entries, leases and provider calls.

## Alternatives Considered

**Encrypt every stream, bucket and object.** Rejected: it recreates the
cold-start cycle [ADR#0033](./0033-key-custody-product-model.md) Decision 5 removes, puts a lease in front of every
projection, replay and tool, and duplicates the server cipher for content no
threat here requires; [ADR#0033](./0033-key-custody-product-model.md) already rejected it for platform state.

**Adopt the AWS Encryption SDK.** Rejected: it has no OpenBao keyring, its key
hierarchy is KMS-centric rather than the `KeyManagement` port, and its Rust
build is a large Dafny-generated tree. Adopted from its [message
format](https://docs.aws.amazon.com/encryption-sdk/latest/developer-guide/message-format.html):
encryption context as AAD, the commitment analysis, and wraps in the envelope.

**Seal stored content with NATS xkeys.** Rejected: an xkey box has no data key,
rotation, generation, veto or AAD, so xkeys serve replies only.

**Seal replies with HPKE now.** Deferred: an xkey box uses XSalsa20-Poly1305,
which FIPS does not approve, and [RFC
9180](https://www.rfc-editor.org/rfc/rfc9180) with an approved suite would
replace it, also for requesters without an xkey, but aws-lc-rs 1.18.1 lacks
HPKE, so that is a separate decision.

**Use age or sops.** Rejected: [age](https://age-encryption.org/) and
[sops](https://github.com/getsops/sops) are file formats with no subject or
message binding, no lease or revocation, and key integrations outside the port.

**Wrap one data key twice, in parallel or nested.** Rejected. In parallel either
wrap yields the key, so the veto is a policy promise; nested, the calls
serialize and an operation against the customer key depends on a platform key.

## Consequences

- `rsworkspace/crates/platform/trogon-sealing` provides `Sealer`, `Opener`, the
  encodings, the lease client and the cache; the secrets service gains the lease
  service, the registry and the invalidation writer.
- Transit is a hard dependency: once a lease expires with Transit unreachable,
  sealing and opening fail closed, large claim-check publishes included.
- Every opener needs a grant, among them the gateway's credential projection,
  session readers and, since claim blobs are always sealed, the
  [ADR#0080](./0080-slack-bolt-nats-receiver.md) TypeScript `NatsReceiver`.
- Backups of sealed content are useless without Transit and, for a customer
  managed tenant, the customer's key.
- The dev compose adds the dev tenant's sealing key beside `local-dev-kek`, and
  the [ADR#0074](./0074-credential-platform-extraction-boundary.md) Decision 3 policies gain grants on sealing keys.
- A FIPS deployment runs without the sealed classes until HPKE is decided,
  keeping TLS and the `aes` server cipher.
- [Key management](../architecture/key-management.md), [key
  custody](../architecture/key-custody.md), [secret
  management](../architecture/secret-management.md), [key
  states](../architecture/key-states.md), [session
  artifacts](../architecture/session-artifacts.md) and the [managed
  key](../glossary/managed-key) entry need updates from their owners.
- Stated limits: an attacker controlling the NATS server; an admitted writer
  dropping, reordering or replaying sealed records, or rolling a KV key back to
  an earlier sealed value; history before a cutover; and a customer veto that
  does not reach OpenBao KV v2 values.

## References

- [ADR#0009: Protocol Buffers Wire Contracts](./0009-protocol-buffers-wire-contracts.md)
- [ADR#0015: Rust TLS Library](./0015-rust-tls-library.md)
- [ADR#0023: Secret Management and Key Custody on OpenBao behind a Platform Secrets Service](./0023-secret-management-and-key-custody-direction.md)
- [ADR#0027: Declared Subject Scope for Decider Stream Resolution](./0027-decider-multi-tenancy-primitive.md)
- [ADR#0030: Customer-Controlled Key Backend Routing](./0030-customer-controlled-key-backend-routing.md)
- [ADR#0033: Key Custody Product Model](./0033-key-custody-product-model.md)
- [ADR#0035: Session Store as a Decider Aggregate on NATS JetStream](./0035-session-store-decider-aggregate.md)
- [ADR#0040: Contract Field Vocabulary: Identifiers, Handles, and Display Labels](./0040-contract-field-vocabulary.md)
- [ADR#0049: Revocation Propagation Latency Target](./0049-revocation-latency-target.md)
- [ADR#0052: Cloud KMS Auto-Unseal Is Mandatory for Production OpenBao](./0052-cloud-kms-production-seal.md)
- [ADR#0057: Decider Command NATS Binding](./0057-decider-command-nats-binding.md)
- [ADR#0059: Stream Incarnation Fencing by Subject Isolation and Sealing](./0059-session-stream-incarnation-fencing.md)
- [ADR#0064: Invariants Are Declared in the Schema Before Anything Enforces Them](./0064-schema-constraints-as-documentation.md)
- [ADR#0068: A Reference Field Says It Is a Reference in Both Its Name and Its Type](./0068-reference-fields-say-they-are-references.md)
- [ADR#0074: Credential Platform Extraction Boundary](./0074-credential-platform-extraction-boundary.md)
- [ADR#0075: API Key Format and Bearer Verifier Construction](./0075-api-key-format-and-verifier-construction.md)
- [ADR#0079: External Secret Sources Resolve Behind the Secrets Service](./0079-external-secret-sources.md)
- [ADR#0080: A Bolt for JavaScript App Consumes the Gateway's Slack Traffic Through a `NatsReceiver` That Dispatches What the Gateway Already Verified and Stored](./0080-slack-bolt-nats-receiver.md)
- [ADR#0081: Every NATS Connection Uses TLS Unless Explicitly Disabled, and Clients Enter Operator-Mode Accounts Only Through Platform Credentials or the Auth Callout](./0081-nats-tls-by-default-and-operator-mode-admission.md)
- [ADR#0082: Inside the Bus the Principal Is Derived from the Subject, and Proof of Possession Is Enforced Only at the External Edge](./0082-bus-principal-from-subject-and-edge-proof-of-possession.md)
- [OpenBao Transit secrets engine API](https://openbao.org/docs/api/secret/transit/) and its 2.6.2 source: [`path_datakey.go`](https://github.com/openbao/openbao/blob/v2.6.2/builtin/logical/transit/path_datakey.go), [`path_keys.go`](https://github.com/openbao/openbao/blob/v2.6.2/builtin/logical/transit/path_keys.go)
- [aws-lc-rs 1.18.1 `aead::RandomizedNonceKey`](https://docs.rs/aws-lc-rs/1.18.1/aws_lc_rs/aead/struct.RandomizedNonceKey.html)
- [zeroize 1.8.2 `Zeroizing`](https://docs.rs/zeroize/1.8.2/zeroize/struct.Zeroizing.html)
