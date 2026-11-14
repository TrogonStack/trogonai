---
title: incident.io
connector:
  name: incident.io
  summary: Bring incident updates into agent workflows
  category: Observability
  color: "#F25533"
  capabilities:
    events: available
---

<ConnectorHeader />

## Events

incident.io delivers webhooks to the gateway, which verifies the signature before publishing. Events land on NATS JetStream for agents to consume.
