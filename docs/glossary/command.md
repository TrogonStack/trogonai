---
term: "Command"
section: "Event sourcing and the decider"
order: 3
---

# Command

An input requesting a change. A command is decided against current state; it is
authorized, admitted, and either rejected or turned into events. See
[ADR#0014](../adr/0014-command-and-query-naming.md) and
[ADR#0026](../adr/0026-command-authorization-principal.md).
