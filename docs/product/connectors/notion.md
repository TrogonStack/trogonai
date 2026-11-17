---
title: Notion
connector:
  name: Notion
  summary: React to page and database changes
  category: Productivity
  icon: notion
  capabilities:
    events: available
---

<ConnectorHeader />

## Events

Notion delivers webhooks to the gateway, which verifies the signature and answers subscription verification requests. Events land on NATS JetStream for agents to consume.
