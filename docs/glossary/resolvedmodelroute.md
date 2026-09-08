---
term: "ResolvedModelRoute"
section: "Agent execution model"
order: 9
---

# ResolvedModelRoute

The immutable, session-owned route that adapts a pinned ModelSelection to an
external provider: provider model identifier, protocol, driver version,
connection, and binding references. Resolving one presumes the platform can read
that pin, which runtime-owned model selection does not currently grant. See
[ADR#0032](../adr/0032-model-route-and-credential-binding.md) and the consequence
recorded in [ADR#0025](../adr/0025-agent-definition-data-ownership.md).
