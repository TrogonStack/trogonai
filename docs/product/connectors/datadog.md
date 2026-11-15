---
title: Datadog
connector:
  name: Datadog
  summary: Route monitor alerts and events to agents
  category: Observability
  icon: datadog
  capabilities:
    events: available
---

<ConnectorHeader />

## Events

Datadog webhooks are unsigned, so the gateway authenticates each delivery with a shared token the operator sets as a custom header on the Datadog webhook. Events land on NATS JetStream for agents to consume.
