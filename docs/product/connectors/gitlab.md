---
title: GitLab
connector:
  name: GitLab
  summary: React to merge requests, issues and pipelines
  category: Developer tools
  icon: gitlab
  capabilities:
    events: available
---

<ConnectorHeader />

## Events

GitLab delivers webhooks to the gateway, which verifies the signing token before publishing. Events land on NATS JetStream for agents to consume.
