---
title: Plaid
connector:
  name: Plaid
  summary: React to account, transaction and item updates
  category: Finance
  color: "#111111"
  capabilities:
    events: available
---

<ConnectorHeader />

## Events

Plaid delivers webhooks to the gateway, which verifies the signed JWT against Plaid's published keys and drops duplicate deliveries. Events land on NATS JetStream for agents to consume.
