# Credential platform research corpus

This corpus is the frozen research input behind the credential platform:
credential vaults, platform API keys, the secret store boundary, and provider
credential custody. The notes were frozen when
[ADR#0074: Credential Platform Extraction Boundary](../../adr/0074-credential-platform-extraction-boundary.md)
and the API key records
[ADR#0075](../../adr/0075-api-key-format-and-verifier-construction.md) through
[ADR#0078](../../adr/0078-api-key-rotation-grace-and-audit-set.md) were
accepted. Where a conclusion here differs from an accepted record in the
[ADR index](../../adr/index.md), the ADR is authoritative.

## Notes

- [API Key Design](./api-key-design.md), the design of the platform's API
  keys: key format, verifier construction, the create and verification flows,
  permissions, rate limits, rerolling, and how keys interact with credential
  vaults.
- [Credential Platform Spec](./credential-platform-spec.md), the contracts and
  boundaries for the credential vault and API key platform: endpoints,
  lifecycle sagas, the authorization matrix, OpenBao operations, runtime
  projection, and UI acceptance.
- [Secret Store Context](./secret-store.md), the implementation context for a
  self-serve secret store: comparison systems, the store interface, adapters,
  encryption, caching, rotation, and runtime resolution.
- [Vercel Connect: Credential Sprawl Research and TrogonAI Adoption Notes](./vercel-connect.md),
  a study of Vercel's credential access broker and what the platform should
  adopt, adapt, and avoid.

## Related

- [Secret Management](../../architecture/secret-management.md)
- [Key Custody](../../architecture/key-custody.md)
- [Provider Credential Custody](../../architecture/provider-credential-custody.md)
- [Supplied Credential Material](../../reference/supplied-credential-material.md)
