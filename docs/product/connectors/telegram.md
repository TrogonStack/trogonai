---
title: Telegram
connector:
  name: Telegram
  summary: Receive bot messages and updates
  category: Communication
  icon: telegram
  capabilities:
    events: available
    channel: available
---

<ConnectorHeader />

## Events

Telegram delivers bot updates to the gateway, which verifies the secret token before publishing. Events land on NATS JetStream for agents to consume.

## Channel

People chat with an agent from a Telegram direct message or group and get the reply in the same chat. `channel-bridge-telegram` turns each update into a channel-neutral event, routes it to the conversation's agent and renders the reply back through the Bot API.

See [Multi-Channel Agent Routing](/architecture/multi-channel-agent-routing).
