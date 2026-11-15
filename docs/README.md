# Documentation

Browse the [documentation home](./index.md) for the full project navigation.
Documents distinguish proposed architecture, normative contract reference and
runtime acceptance evidence.

## Development verification

[Verify scheduler decisions with Hydro](./how-to/verify-scheduler-with-hydro.md)
explores client command orderings and exercises the production scheduler domain
deciders for each order. Its scope is separate from broker and processor recovery.

## Workload identity

The proposed [workload identity profile](./reference/workload-identity.md)
defines attested bootstrap, issuer isolation, rotating credentials, independent
authorization and provider token protection. Its implementation and verification
checklists distinguish runtime foundations from deployment acceptance. The
governing [ADR#0098](./adr/0098-attested-workload-identity.md) remains draft.

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
