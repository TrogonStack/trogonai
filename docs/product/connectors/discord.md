---
title: Discord
connector:
  name: Discord
  summary: React to messages and server activity
  category: Communication
  icon: discord
  capabilities:
    events: available
    channel: planned
---

<ConnectorHeader />

## Events

The gateway holds a Discord gateway (WebSocket) session and publishes each event it receives. Events land on NATS JetStream for agents to consume.

## Channel

Chatting with an agent from Discord is planned. It would be a new edge on the same channel-neutral routing that Telegram uses, with no change to agents.

See [Multi-Channel Agent Routing](/architecture/multi-channel-agent-routing).
