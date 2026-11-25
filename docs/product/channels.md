---
title: Channels
aside: false
pageClass: wide-page
product:
  name: Channels
  tagline: Talk to agents from the chat apps people already use.
  order: 3
  status: available
  icon: messages-square
  color: "#2563EB"
---

<ProductHeader />

People message an agent from a chat app and get a reply in the same
conversation. Everything between the chat app and the agent is
channel-neutral, so adding a channel adds an edge, not a new agent.

<ConnectorGallery capability="channel" :filters="false" />

## Add a channel to this page

A channel is a [connector](/product/connectors/) that declares the `channel`
capability. Set it in the connector's page under
`docs/product/connectors/<slug>.md`:

```yaml
connector:
  capabilities:
    events: available
    channel: planned # available, preview or planned
```

## Learn more

- [Multi-Channel Agent Routing](/architecture/multi-channel-agent-routing)
- [Slack Acknowledgement Responses](/architecture/slack-acknowledgement-responses)
