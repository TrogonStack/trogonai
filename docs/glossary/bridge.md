---
term: "Bridge"
section: "Protocols and transports"
order: 6
---

# Bridge

The component that decodes protocol messages into typed SDK structs, routes them
over NATS, and re-serializes them at the far side. The bridge owns callback
traits at the SDK boundary. See
[ADR#0020](../adr/0020-acp-sdk-1x-boundary-and-bridge-traits.md) and
[ACP Conformance](../architecture/acp-conformance.md).
