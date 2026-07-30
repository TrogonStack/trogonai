---
term: "ModelSelection"
section: "Agent execution model"
order: 4
---

# ModelSelection

An exact, versioned model catalog pin plus deterministic parameters, distinct
from a display name, mutable alias, or provider credential. The shipped contract
carries no such platform-owned pin: model selection lives in the runtime-owned
settings on AgentConfiguration, so this term names the intended platform concept
that route admission and credential binding would need, not a field that exists
today. See [ADR#0031](../adr/0031-agent-implementation-and-session-plan.md) and
the consequence recorded in
[ADR#0025](../adr/0025-agent-definition-data-ownership.md).
