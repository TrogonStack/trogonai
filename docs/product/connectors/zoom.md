---
title: Zoom
connector:
  name: Zoom
  summary: React to meetings, recordings and transcripts
  category: Communication
  icon: zoom
  capabilities:
    events: available
---

<ConnectorHeader />

## Events

Zoom delivers webhooks to the gateway, which verifies the signature and answers URL validation challenges. Events land on NATS JetStream for agents to consume.

See [Receive Meeting Transcript Notifications](/how-to/receive-meeting-transcript-notifications) for a transcript example.
