# Workload identity configuration contracts

These design contracts describe Trogon's governed identity configuration and
its relationship to workflow execution. They complement the implemented
`platform.workload_identity` registration and audit messages. They do not
implement a trust controller, issuer, provider exchange or workflow admission
service.

The proposed behavior is explained in the
[federation design](../../../../docs/research/workload-identity/federation.md)
and [adapter roadmap](../../../../docs/research/workload-identity/adapter-roadmap.md),
under draft [ADR#0053](../../../../docs/adr/0053-external-oidc-federation-surface.md)
and [ADR#0098](../../../../docs/adr/0098-attested-workload-identity.md).
Those records retain their current statuses and production acceptance gates.

## Ownership

Trust registrations govern foreign-domain bundles and configured OIDC
verification profiles. Subject bindings map an exact verified external subject
to admitted platform authority. Recipient bindings govern the external resource
and provider identity a trusted executor may use. Execution profiles select
qualified runtime identity requirements for a workflow. Issuer and signing
profiles retain their separate custody and purpose boundaries.

Tenant and project placement comes from authenticated context and admitted
configuration. A subject URI, a copied reference or a workflow ID cannot choose
another tenant's authority. A valid foreign credential does not automatically
enroll a local SPIFFE workload.

## Configuration identity

References bind an exact scope, opaque configuration identity, revision,
recovery fence and artifact digest. Configuration artifacts contain the exact
immutable definition bytes retained by their owner. Their digest excludes the
outer reference and wrapper, avoiding a recursive self-hash.

Admission and replay must verify the artifact hash, decode the exact supported
definition type/version and compare the decoded definition with the typed
projection. Re-encoding a protobuf projection cannot replace verification of
the original bytes. Message-local validation cannot perform these lookups or
establish provenance.

Lifecycle, admission and external qualification remain distinct. Health or a
qualification reference cannot grant permission. Changes that widen trust need
the appropriate independent approval, while withdrawal must take effect before
later bookkeeping can delay it. Restore must independently advance the recovery
fence and revoke prior writers.

## Existing contracts remain authoritative

- `platform.workload_identity.RegistrationBinding` and `RegistrationSnapshot`
  own the effective X.509 registration projection.
- `CredentialActivation` records acknowledged activation intent, while the
  material owner still performs the final currentness check.
- Workflow `RunAuthority`, run limits and launch admission retain workflow
  delegation and execution provenance.
- `connections.grants.ConnectionGrantTerms` retains Session-attempt resource
  authority and confirmation-key binding.
- Key-custody backend registrations and access profiles retain business key
  routing. Infrastructure signing custody does not become a business `KeyRef`.

SPIFFE Workload API messages, SPIRE plugin configuration, cloud IAM APIs and
provider credential formats remain upstream protocols. Qualification references
bind the selected implementation and evidence; these contracts do not duplicate
those protocols or imply that an available upstream plugin is qualified here.

## Validation and implementation boundary

Use the repository's shared fixture runner:

```sh
mise run proto:validate -- --suite workload_federation
mise run proto:validate -- --suite workflows
```

Buf compilation, lint and fixture validation cover syntax and message-local
constraints. Signature verification, exact artifact bytes, provider response
interpretation, cross-record policy, state transitions, live revocation,
credential custody and deployed isolation require separate implementation and
interoperability tests.

The package remains excluded from runtime Rust generation while its contracts
are under design. Promotion requires the explicit validation-enforcement
decision described by accepted
[ADR#0064](../../../../docs/adr/0064-schema-constraints-as-documentation.md).
No provider assertion, access token, private key or usable cloud credential
belongs in durable configuration, events, query results or diagnostic evidence.
