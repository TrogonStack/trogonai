# Portable workload identity research

These proposals describe how workload identity, key custody and external
federation can share a portable authority model. AWS, Google Cloud, Azure,
GitHub and Kubernetes are optional integration profiles in the proposed
design. Trogon retains application identity, tenant ownership, authorization,
audit and failure semantics.

The research baseline is repository `main` at
[`8d541e6`](https://github.com/TrogonStack/trogonai/tree/8d541e6a868e9016405b3846db56bb5b983243a0),
reviewed on 2026-11-09. The documents distinguish source observations from
proposed contracts and acceptance still to execute. They do not change ADR
status or establish provider support.

## Reading paths

- Read [Workload identity and portable trust](../../architecture/workload-identity.md)
  for workload terminology, SPIRE responsibilities, activation and authorization.
- Use the [runtime reference](../../reference/workload-identity.md) and
  [deployment procedure](../../how-to/deploy-workload-identity.md) for the
  implemented foundation, exact limits and deployment gates.
- Use [registration operations](../../how-to/operate-workload-registration.md)
  and [trusted-host integration](../../how-to/integrate-workload-identity.md)
  for approval publication, recovery and retained identity during network work.
- Use the [configuration contract reference](../../reference/workload-identity-configuration.md)
  for protobuf ownership, persisted workflow identity requirements and run admission.
- Use the [portable wrapping contract](../../reference/key-wrapping.md) for the
  Rust domain foundation, exact binding encoding and conformance boundaries.
- Review the [federation proposal](./federation.md) for native SPIFFE trust,
  inbound external workload admission, outbound provider exchange, credential
  custody and negative acceptance cases. F01 through F10 organize the work.
- Review the [identity and key-custody adapter roadmap](./adapter-roadmap.md)
  for the source inventory, ADR reconciliation, adapter catalogue, dependencies
  and delivery gates. P00 through P44 are work-package labels; gaps between
  labels group the work and do not reserve ADR numbers.

## Decisions that remain open

| Decision | Governing record or proposal |
| --- | --- |
| Portable environment/issuer qualification and foreign-domain authority | Draft [ADR#0098](../../adr/0098-attested-workload-identity.md) and the federation proposal. SPIRE is the initial implementation; equivalent issuer behavior needs qualification. |
| Independently operated production bootstrap/seal profile | Accepted [ADR#0052](../../adr/0052-cloud-kms-production-seal.md) currently mandates cloud KMS auto-unseal with a restricted-egress exception. Portability requires an explicit amendment or supersession. |
| Wrapping and custody adoption | Draft [ADR#0030](../../adr/0030-customer-controlled-key-backend-routing.md) and [ADR#0033](../../adr/0033-key-custody-product-model.md), reconciled with accepted [ADR#0083](../../adr/0083-sealed-content-envelope.md). |
| External assertion issuance and recipient compatibility | Draft [ADR#0053](../../adr/0053-external-oidc-federation-surface.md), including SPIRE helper reuse and the Entra JWKS evidence question. |
| Provider-free application custody boundary | Accepted [ADR#0074](../../adr/0074-credential-platform-extraction-boundary.md); the extraction and opaque-ID migration remain implementation dependencies. |

The recommended first federation proof uses independently controlled local
SPIRE trust domains without a cloud, GitHub or Kubernetes prerequisite. It
must demonstrate allowed access, authenticated-but-unauthorized refusal,
rotation, root removal and established-session withdrawal. Provider profiles
then qualify their own algorithms, claims, IAM and residual credential windows.

Acceptance of a proposal follows
[ADR#0000](../../adr/0000-adr-process.md), including independent maintainer
signoff. Implementation, real-provider qualification and deployed operational
acceptance remain separate milestones after the decision.
