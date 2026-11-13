---
title: Linear
connector:
  name: Linear
  summary: Receive webhooks for issues, comments and more
  category: Productivity
  icon: linear
  capabilities:
    events: available
    actions: planned
---

<ConnectorHeader />

## Events

Linear delivers webhooks to the gateway, which verifies the signature before publishing. Events land on NATS JetStream for agents to consume.

## Actions

Agents will reach the Linear API through a project-owned connection. The [Linear Connector Profile](/reference/connector-linear-profile) is the reviewed candidate for that connection; it is not yet an enabled release.
