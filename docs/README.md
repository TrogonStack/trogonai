# Documentation

Browse the [documentation home](./index.md) for the full project navigation.
Documents distinguish proposed architecture, normative contract reference and
runtime acceptance evidence.

## Development verification

[Verify scheduler decisions with Hydro](./how-to/verify-scheduler-with-hydro.md)
explores client command orderings and exercises the production scheduler domain
deciders for each order. Its scope is separate from broker and processor recovery.

## Workload identity

Start with [Workload identity and portable trust](./architecture/workload-identity.md)
for workload terminology, SPIRE responsibilities and independent authorization.
The [runtime reference](./reference/workload-identity.md) and
[deployment procedure](./how-to/deploy-workload-identity.md) distinguish
implemented foundations from deployment acceptance. The governing
[ADR#0098](./adr/0098-attested-workload-identity.md) remains draft.

The [configuration contract reference](./reference/workload-identity-configuration.md)
maps existing and proposed protobuf owners, saved workflow identity requirements,
run admission, exact artifacts and approval boundaries.

Use [Operate workload registration](./how-to/operate-workload-registration.md)
to connect a protected approval controller and recover a producer, and
[Integrate workload identity](./how-to/integrate-workload-identity.md) to retain
current identity through a trusted host's handshakes and established sessions.

The [portable identity research](./research/workload-identity/README.md)
connects the [federation proposal](./research/workload-identity/federation.md)
with the [identity/KMS adapter roadmap](./research/workload-identity/adapter-roadmap.md).
It records optional environment/provider profiles, existing implementation,
ADR decisions still needed and acceptance gates. These proposals do not change
ADR status or establish deployed adapter support.

## Third-party connections

- Understand the [architecture](./architecture/third-party-connections.md).
- Look up [security defaults](./reference/connector-security-profile.md),
  [resource parents](./reference/connector-resource-parents.md),
  [account identity](./reference/connector-account-identity.md),
  [MCP authorization](./reference/connector-mcp-authorization.md),
  [control API](./reference/connector-control-api.md),
  [HTTP transport](./reference/connector-http-transport.md),
  [binding integration](./reference/connector-binding-integration.md),
  [relay recovery](./reference/connector-relay-recovery.md) and
  [conformance](./reference/connector-conformance.md).
- [Add a provider](./how-to/add-a-connector-provider.md),
  [connect/reconnect](./how-to/connect-a-provider.md),
  [integrate HTTP use](./how-to/integrate-connector-http.md) or
  [operate recovery and erasure](./how-to/operate-connectors.md).

The linked connector ADRs remain drafts. Contract and fixture validation does
not establish a deployed service or provider approval.
