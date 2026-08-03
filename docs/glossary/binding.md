---
term: "Binding"
section: "Channels and conversations"
order: 4
---

# Binding

One [KV bucket](./kv-bucket) entry mapping an [endpoint](./endpoint) to a
[conversation](./conversation) id, and nothing more than that. A message
arrives, the bridge reads the entry, and either follows it or, when the entry is
absent, treats the message as the start of a new conversation: routing policy
runs once, the conversation is created, and the entry is written.

The binding is the routing record itself rather than a layer in front of one,
and it is sticky, so operator config changes affect new conversations only and a
live conversation never silently changes agents. See
[Multi-Channel Agent Routing](../architecture/multi-channel-agent-routing.md).
