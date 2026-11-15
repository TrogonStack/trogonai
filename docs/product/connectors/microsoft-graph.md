---
title: Microsoft Graph
connector:
  name: Microsoft Graph
  summary: Receive Teams, Outlook and calendar change notifications
  category: Productivity
  color: "#0078D4"
  capabilities:
    events: available
---

<ConnectorHeader />

## Events

Microsoft Graph delivers change notifications to the gateway, which checks the client state before publishing. Events land on NATS JetStream for agents to consume.
