---
title: Google Chat
connector:
  name: Google Chat
  summary: Receive Google Chat app interactions through Cloud Pub/Sub
  category: Communication
  icon: googlechat
  capabilities:
    events: preview
---

<ConnectorHeader />

## Events

Receive messages that @mention your Chat app in a space and direct messages
sent to the app. Google Chat publishes these interactions to a Cloud Pub/Sub
topic. The gateway pulls them from a subscription and publishes them to NATS
JetStream for consumers.

To connect a space, configure your Chat app to publish to Cloud Pub/Sub, add
the app to the space, and configure the gateway to read from the topic's pull
subscription. [Google Chat setup](https://developers.google.com/workspace/chat/quickstart/pub-sub) · [TrogonAI Pub/Sub connector](/product/connectors/google-pubsub)
