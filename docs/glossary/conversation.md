---
term: "Conversation"
section: "Channels and conversations"
order: 3
---

# Conversation

The durable context an agent works in, and the root object of the channel
domain: [endpoints](./endpoint) point at conversations, never the reverse. A
conversation holds a sticky `agent_id`, chosen once by routing policy when the
conversation is created and never changed by later operator config edits, plus a
pointer to the current [session](./session).

A conversation outlives its sessions. Sessions belong to the agent and churn for
ordinary reasons (reset, expiry, agent restart); replacing one never re-runs
routing policy and never changes the bound agent. See
[Multi-Channel Agent Routing](../architecture/multi-channel-agent-routing.md).
