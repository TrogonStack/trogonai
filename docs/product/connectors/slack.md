---
title: Slack
connector:
  name: Slack
  summary: Receive messages, interactions and slash commands
  category: Communication
  color: "#4A154B"
  capabilities:
    events: available
    channel: planned
---

<ConnectorHeader />

## Events

Slack delivers Events API payloads, interactive payloads and slash commands over an HTTP webhook or Socket Mode. Events land on NATS JetStream for agents to consume.

See [Multi-Channel Agent Routing](/architecture/multi-channel-agent-routing) and [Slack Acknowledgement Responses](/architecture/slack-acknowledgement-responses).

## Channel

Chatting with an agent from Slack is planned. It would be a new edge on the same channel-neutral routing that Telegram uses, with no change to agents.
