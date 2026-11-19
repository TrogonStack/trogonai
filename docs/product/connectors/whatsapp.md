---
title: WhatsApp
connector:
  name: WhatsApp
  summary: Receive messages sent to a WhatsApp Business number
  category: Communication
  icon: whatsapp
  capabilities:
    events: preview
---

<ConnectorHeader />

## Events

The gateway receives signed WhatsApp Business Cloud API webhooks and publishes
inbound messages for the configured Messaging account and business phone number
to JetStream. This source is in Preview pending live delivery verification
with Meta.

[Set up WhatsApp message ingestion](/how-to/receive-whatsapp-messages).
