# Workload identity configuration contracts

This reference maps the protobuf contracts for the proposed workload identity,
federation and adapter configuration model. The
[architecture explanation](../architecture/workload-identity.md) introduces
the concepts. The [federation proposal](../research/workload-identity/federation.md)
and [adapter roadmap](../research/workload-identity/adapter-roadmap.md) record
the decisions and implementation gates.

The new configuration contracts remain design schemas under draft
[ADR#0053](../adr/0053-external-oidc-federation-surface.md) and
[ADR#0098](../adr/0098-attested-workload-identity.md). They do not accept those
ADRs or establish a running configuration service, issuer or provider adapter.

## Contract ownership

| Concern | Protobuf owner | Boundary |
| --- | --- | --- |
| Effective local X.509 registration | `platform/workload_identity/v1alpha1` | Exact SPIFFE identity, platform mapping, approved root fingerprints, finite registration lease and recovery fence. |
| Activation and authenticated peer evidence | `CredentialActivation`, `WorkloadPeerEvidence` | Public attribution and material generations. Activation intent still requires the material owner's final live check. |
| Managed foreign trust and external subject admission | `platform/workload_federation/v1alpha1` | Domain-to-bundle relationship, configured OIDC verifier, literal claims and exact platform principal binding. |
| External recipient selection | `RecipientBindingDefinition` and provider recipient profiles | Source identity and operation to one provider account, role, client, audience and resource. |
| Signing and assertion issuance | `SigningProfileDefinition`, `IssuerProfileDefinition` | Purpose, algorithm, custody qualification and separate public discovery surface. Business key references remain separate. |
| Execution environment and identity requirements | `ExecutionProfileDefinition` | Qualified issuer/environment artifacts, constrained attestation, allowed source identities, recipients and finite admission lease. |
| Workflow definition and run provenance | `workflows/definitions` and `workflows/runs` | Exact definition bytes/version/digest, existing `RunAuthority`, run limits and identity-aware admission. |
| Session-attempt resource use | `connections/grants/v1alpha1` | Existing Connection grant, live permit, authority ceiling, recipient and confirmation-key requirements. |
| Business key backend configuration | `platform/key_custody/v1alpha1` | Existing immutable key references, provider access profiles, registration lifecycle and audit outcomes. |
| Sensitive key-wrapping wire | `secrets/exchange/v1alpha1/key_wrapping*.proto` | Typed authenticated binding, explicit exact/default key selection, opaque wrapped values and sealed core request/reply. |
| External read-only secret sources | `secrets/sources/v1alpha1` | Source/entry registration behind existing `SecretRef`, private provider configuration, metadata validation and safe resolution/revocation evidence. |

Lifecycle commands, events, state and typed configuration reads accompany the
configuration owners. Actor and occurrence time remain in the event envelope.
The existing Decider transport owns command identity and expected stream
revision. A valid command shape does not authorize its sender or establish a
legal state transition.

## What a workflow saves

A workflow's identity requirement references an admitted execution profile.
The profile pins its immutable configuration artifact and the identity sources
and recipients it may use. Shared trust registrations can serve multiple
workflows in their approved scope.

The definition remains distinct from a run. At admission, the trusted owner
resolves the actual workflow/plan version, authenticated executor, initiating
authority, profile and current policy. The run retains public evidence and
references to that decision. A workflow ID or a copied admission record cannot
grant access.

An identity-aware workflow uses an explicit definition contract version. Older
definitions retain their prior contract; they cannot silently carry a new
identity requirement. Readers must verify the version against the exact
definition bytes and refuse unsupported behavior before execution.

The workflow executor's identity is separate from the identity and confirmation
key of a Session worker it launches. Existing Session and Connection admission
remain necessary. Reusing `RunAuthority` carries delegated authority forward;
it does not turn the workflow executor's SVID into permission for every worker.

## Exact configuration and approval

An immutable configuration reference includes its scope, identity, revision,
recovery fence and definition-artifact digest. The artifact contains the exact
bytes of the owning definition type, excluding the outer reference. This avoids
hashing a structure that contains its own digest.

Admission and replay verify the hash and supported type/version, then compare
the decoded artifact with its typed projection. They must not replace the
original bytes with a protobuf re-encoding and assume universal canonicalization.

Approval and validation receipts bind the owning configuration kind and
operation purpose as well as the exact scope/version. A receipt for a trust
registration cannot activate a recipient, signer or execution profile.
Verification also checks the authentic independent approver, current owner
state and receipt validity. Message-local equality checks do not perform that
verification.

Withdrawal changes local admission immediately under the selected profile.
Bookkeeping or audit recovery cannot turn an old grant back on. Restoring
configuration storage requires an independently advanced recovery fence and
revoked prior writers; replaying a restored counter is insufficient.

## Proposed wire bounds

These ceilings bound the new design messages. They are not measured capacity
or provider protocol limits. A selected profile can choose smaller limits.

| Data | Ceiling |
| --- | --- |
| Exact configuration artifact or pinned public JWKS | 256 KiB |
| Bootstrap public certificate | 16 KiB, with at most 16 roots per endpoint |
| Inbound OIDC assertion | 64 KiB |
| Parsed JWKS and claims | 128 keys and 64 claims |
| OIDC request timeout and clock-skew allowance | 30 seconds and 120 seconds, respectively |
| Literal claim requirements | 32 requirements, each with at most 32 accepted values |
| Configuration list page and opaque continuation | 100 records and 4096 bytes |
| Execution identity admission lease | 330 seconds, also bounded by original source and operation authority |

Request parsing, authentication, key/certificate interpretation and total
transport/concurrency bounds remain enforcement obligations. Accepting a bounded
byte field does not prove that its contents are valid verification material.

## Credentials and provider behavior

Durable configuration and events contain public trust material, typed policy
and bounded metadata. Assertions, SVID private keys, provider tokens and usable
cloud credentials remain inside the trusted execution/custody boundary.

Outbound exchange distinguishes original authority, local permitted use and
the provider's actual credential expiry. AWS web identity, for example, has a
900-second minimum session while the local registration ceiling is 330 seconds.
Stopping local broker use does not revoke an issued AWS credential.

Provider profiles select exact trust and resource semantics. GCP certificate
binding remains an explicit Preview profile requiring actual resource-path
qualification. Entra's conservative RSA-only published-key profile remains
subject to the evidence question recorded in the federation proposal. Profile
selection is not a claim that interoperability has been executed.

## Secret-source and key-wrapping boundaries

External source registrations follow draft
[ADR#0079](../adr/0079-external-secret-sources.md). AWS Secrets Manager,
Google Secret Manager and customer OpenBao KV v2 remain read-only sources
behind the existing opaque `SecretRef`. Their private administrative definitions
hold provider locators and non-secret access profiles. Consumer-facing
references and ordinary events do not carry those locators or resolved values.

Registration validates metadata access. It cannot establish that the first
material read will succeed. Resolution follows `AWSCURRENT`, `latest` or the
current KV version, without falling back to an older version or a platform-held
copy. Provider consistency, local cache expiry and reference withdrawal are
different observations. Authentication bootstrap must not depend on the same
source-backed reference it needs to unlock.

The key-wrapping protocol follows draft
[ADR#0030](../adr/0030-customer-controlled-key-backend-routing.md) and accepted
[ADR#0083](../adr/0083-sealed-content-envelope.md). Its initial material suite
is 32 bytes. The [Rust domain contract](./key-wrapping.md) implements the portable
values and operation ports; encrypted transport and real adapters remain separate.
`AuthenticatedBinding` uses explicit versioned length-prefixed
identifiers derived from authoritative tenant/resource/purpose state.
`WrappedDek` remains an opaque versioned adapter payload.

Raw data keys occur only in ephemeral inner messages. Outer requests and
successful responses carry authenticated xkey-sealed payloads over core NATS.
They retain the original operation, binding, holder/service keys and deadline.
An explicitly selected `KeyRef` is validated and cannot fall back to a default
or another backend. An explicit default selection returns the actual immutable
key/wrapped-value pair for the data owner to persist atomically.

These schemas do not implement encryption, key erasure, provider adapters or
sealing leases. The narrow sealing lease exception remains separate from the
ordinary wrap/unwrap wire. No sensitive inner message may enter JetStream,
durable commands, events or diagnostic logs.

## Evidence boundary

Schema validation establishes supported shape and message-local invariants.
It cannot verify signatures, artifact contents, current grants, cross-stream
history, provider IAM, actual HSM protection or established-session withdrawal.
Those remain implementation and deployment acceptance work.

## Formats retained by their owners

Protobuf coverage does not introduce replacement formats for existing protocols:

- SPIFFE Workload API, SVID and federation bundle formats remain the upstream
  interfaces. Trogon's schemas govern which relationships and identities are
  admitted.
- SPIRE plugin settings and OpenBao seal configuration remain deployment
  artifacts. Qualification references bind the selected artifact and evidence;
  business key-management calls do not configure their own bootstrap.
- The implemented live egress-policy reader retains its authenticated JSON KV
  document and embedded TOML policy. An authoritative publisher must satisfy
  that [existing contract](./workload-identity.md#live-egress-authority);
  a second protobuf wire format is not introduced here.
- The data owner's sealed-content envelope and adapter ciphertext codecs retain
  their explicit versions under
  [ADR#0083](../adr/0083-sealed-content-envelope.md). The key-wrapping messages
  do not create a competing payload-encryption format.
- Provider HTTP APIs and cloud IAM documents remain provider protocols.
  Typed recipient and access profiles constrain their use without exposing an
  arbitrary SDK request or credential passthrough.

## Generation and validation

The new configuration package remains excluded from runtime Rust generation.
Existing workflow design packages retain their exclusion. Promotion requires
the explicit validation-enforcement decision in accepted
[ADR#0064](../adr/0064-schema-constraints-as-documentation.md).

Use the shared protobuf fixture runner and the repository's Buf checks. The
fixture suites provide positive controls and deliberate invalid cases; a parser
or setup failure is not a successful security refusal.
