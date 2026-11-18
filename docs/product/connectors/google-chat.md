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
the app to the space, and create a dedicated pull subscription on that topic for
the gateway. If you run Google's quickstart sample, give it a separate
subscription on the same topic so the gateway receives its own copy of each
event. [Google Chat setup](https://developers.google.com/workspace/chat/quickstart/pub-sub) · [TrogonAI Pub/Sub connector](/product/connectors/google-pubsub)
