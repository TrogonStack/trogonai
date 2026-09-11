---
term: "ModelSelection"
section: "Agent execution model"
order: 4
---

# ModelSelection

An exact, versioned model catalog pin plus deterministic parameters, distinct
from a display name, mutable alias, or provider credential. For platform-managed
model access, a supported pinned adapter derives and validates this projection
from the runtime-owned settings. Session admission records it and resolves its
provider route without substituting another model. It is not an independently
authored universal AgentConfiguration field. A runtime that cannot expose and
enforce this information cannot use that capability. See draft
[ADR#0032](../adr/0032-model-route-and-credential-binding.md) and
[ADR#0062](../adr/0062-runtime-owned-settings-and-platform-declarations.md).
