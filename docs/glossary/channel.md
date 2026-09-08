---
term: "Channel"
section: "Channels and conversations"
order: 0
---

# Channel

A messaging platform a human reaches an agent through (Telegram, Discord, Slack,
the CLI). Channels carry no intelligence of their own: a channel
[bridge](./bridge) translates between the platform's shape and the neutral
inbound event and render commands, and everything downstream of that translation
is channel-blind. `channel` is also the first token of an
[endpoint](./endpoint). See
[Multi-Channel Agent Routing](../architecture/multi-channel-agent-routing.md).
