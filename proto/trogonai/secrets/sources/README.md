# External secret source contracts

These design contracts specialize draft [ADR#0079](../../../../docs/adr/0079-external-secret-sources.md).
They register read-only AWS Secrets Manager, Google Secret Manager and customer
OpenBao KVv2 sources behind the existing opaque `secrets.SecretRef`.

## Contract boundaries

| Surface | Responsibility |
| --- | --- |
| `SecretSourceDefinition` and `SecretSourcePrivateConfiguration` | Provider access profiles and selected entry locators in protected source-owner administrative state; exact retained Definition bytes and hash |
| `RegisterSecretSourceRequest` | Authenticated private administrative submission, metadata-only validation and service-minted entry references |
| `SecretSourceRegistration`, control events and state | Locator-free immutable registration, entry/reference lineage, terminal revocation and safe observation metadata |
| Get/list queries | Scoped bounded metadata reads and a separate authorized owner-only private configuration readback |
| `SourceResolutionEvidence` | Observed current version, actual value-validation receipt, finite local cache bound or typed failure |
| `SecretReferenceDegraded` and revocation evidence | Safe reference/category metadata for invalidation and held-session handling |

An entry always reads `AWSCURRENT`, `latest` or the current KVv2 version. There
is no version pinning, provider write/rotate/delete verb, stale/older-version
fallback or copy into platform storage. A new source/security boundary or
entry receives new registration/reference identities; existing references are
never retargeted. Registration validates metadata only. Material-read and
consumer-specific value validation can still fail on first resolve.

Usable material continues through the existing trusted
`secrets.exchange.ResolveForTrustedUse` and `SealedAccess` path. There is no
new raw-reference material RPC, no inline source credential and no credential
payload in source events, state or audit. A bootstrap credential reference is
accepted only when the secrets owner proves platform-stored provenance and
absence of a source dependency cycle.

## Acceptance boundary

Buf and Protovalidate fixtures establish schema shape and message-local rules.
Runtime admission must also bound aggregate request/reply bytes and concurrency;
pages stop at the transport budget and resume through an authenticated cursor.
Passing fixtures does not establish authenticated ownership, exact artifact decoding/hashing,
metadata/material permissions, endpoint egress safety, provider consistency,
credential validation, cache invalidation, lifecycle replay or session closure.
The 330-second local cache ceiling does not bound provider rotation visibility
or independently end an already authenticated remote session. Source-side
changes produce no automatic platform event in this initial profile; explicit
platform reference revocation uses the existing invalidation path.

The contracts remain excluded from runtime code generation. No adapter,
production source registration or deployed permission is established here.
