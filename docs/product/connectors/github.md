---
title: GitHub
connector:
  name: GitHub
  summary: React to pull requests, issues and workflow runs
  category: Developer tools
  icon: github
  capabilities:
    events: available
---

<ConnectorHeader />

## Events

GitHub delivers webhooks to the gateway, which verifies the signature before publishing. Events land on NATS JetStream for agents to consume.
