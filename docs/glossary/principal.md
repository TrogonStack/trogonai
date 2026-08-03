---
term: "Principal"
section: "Channels and conversations"
order: 2
---

# Principal

The identity behind one or more [endpoints](./endpoint). An endpoint that
resolves to no principal is rejected at the [bridge](./bridge), and that
rejection is the entire access-control mechanism for channels: there is no
separate allowlist. Linking one person's Telegram and Discord endpoints to a
single principal is what allows a [conversation](./conversation) to continue
across channels.

For a group chat the linked principal stands for the room rather than for a
person, since the room is one endpoint. That imprecision is recorded, with its
consequences, in
[Multi-Channel Agent Routing](../architecture/multi-channel-agent-routing.md).

Distinct from the decider's command-authorization principal
([ADR#0026](../adr/0026-command-authorization-principal.md)), which authorizes
command execution at a different layer.
