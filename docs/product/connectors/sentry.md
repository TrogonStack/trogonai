---
title: Sentry
connector:
  name: Sentry
  summary: Route errors and issue alerts to agents
  category: Observability
  icon: sentry
  capabilities:
    events: available
---

<ConnectorHeader />

## Events

Sentry delivers webhooks to the gateway, which verifies the signature with the integration's client secret. Events land on NATS JetStream for agents to consume.
