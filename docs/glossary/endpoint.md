---
term: "Endpoint"
section: "Channels and conversations"
order: 1
---

# Endpoint

One place messages arrive and leave, addressed by three tokens: `channel` (which
platform), `account` (which of our bots on that platform), and `peer` (which chat
on the far side). Joined with dots it reads
`telegram.mybot.-1001234567890`, and that one string serves as both a
[KV bucket](./kv-bucket) key and the tail of a NATS subject, which is why the
tokens are restricted to characters that both accept.

An endpoint addresses a **chat, not a person**: everyone in a group shares one
endpoint, so authorizing an endpoint authorizes the room. Many endpoints can
point at one [conversation](./conversation), which is what makes a conversation
cross-channel. See
[Multi-Channel Agent Routing](../architecture/multi-channel-agent-routing.md).
