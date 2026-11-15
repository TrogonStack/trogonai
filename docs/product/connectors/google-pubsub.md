---
title: Google Cloud Pub/Sub
connector:
  name: Google Cloud Pub/Sub
  summary: Stream Workspace and Cloud notifications to agents
  category: Cloud
  icon: googlecloud
  capabilities:
    events: available
---

<ConnectorHeader />

## Events

The gateway pulls from a Pub/Sub subscription with streaming pull, so no public endpoint is needed. Events land on NATS JetStream for agents to consume.

See [Receive Meeting Transcript Notifications](/how-to/receive-meeting-transcript-notifications) for a Google Meet example.
