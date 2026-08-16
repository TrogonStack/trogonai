---
term: "Command principal"
section: "Event sourcing and the decider"
order: 17
---

# Command principal

Who is submitting a [command](./command): a kind (agent, person, or service), a
stable id, and the claims the caller was granted. Defined by
[ADR#0026](../adr/0026-command-authorization-principal.md) and carried into
execution alongside the command itself, where an opt-in authorizer decides
whether that principal may run it. An execution with no authorizer configured
never asks; an execution with one refuses a command that carries no principal at
all.

The principal is an input the runtime trusts rather than a claim it verifies. It
is only as trustworthy as whatever produced it, which is why deriving one from a
verified credential is a separate concern that lives at the edge.

Distinct from the channels [principal](./principal), which is the identity behind
an endpoint and governs message routing rather than command execution.
