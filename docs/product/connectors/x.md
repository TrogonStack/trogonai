---
title: X
connector:
  name: X
  summary: Receive account activity from X
  category: Communication
  icon: x
  capabilities:
    events: available
---

<ConnectorHeader />

## Events

X delivers account activity webhooks to the gateway, which answers CRC challenges and verifies each signed delivery. Events land on NATS JetStream for agents to consume.
