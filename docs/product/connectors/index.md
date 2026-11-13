---
title: Connectors
aside: false
pageClass: wide-page
product:
  name: Connectors
  tagline: Let agents hear from and act in the tools your team already uses.
  order: 1
  status: available
  icon: plug
  color: "#6F4CFF"
guarantees:
  - icon: shield-check
    title: Proven before it is published
    text: Every request passes the provider's own check, a signature, token or signed key, before any agent can see it.
  - icon: copy
    title: A retry arrives once
    text: When the provider sends a delivery id, a redelivered webhook is recognized and published only once.
  - icon: inbox
    title: Nothing dropped silently
    text: A body the gateway cannot route is published with the reason attached, so you can see it and act on it.
  - icon: timer
    title: Slack gets its answer in time
    text: Slash commands and modal submissions are answered inside the three seconds Slack allows.
  - icon: users
    title: Every account kept apart
    text: Each named integration has its own route, its own subjects and its own stream.
  - icon: key-round
    title: Secrets by reference
    text: Configuration names an environment variable or a secret store entry. It never holds the secret itself.
  - icon: server
    title: One process to run
    text: One HTTP port for every webhook, plus the long-lived connections some providers need, with health probes.
progress:
  - name: Webhook gateway with checks for each provider
    status: built
    note: Signatures, tokens, timestamp windows and signed keys, verified before anything is published.
  - name: Long-lived connections
    status: built
    note: A Discord gateway session and Google Pub/Sub streaming pull run inside the same process.
  - name: Several accounts per provider
    status: built
    note: Named integrations, each with its own route, subject prefix and JetStream stream.
  - name: Interactive Slack responses
    status: built
    note: Slash commands, modal submissions and option menus answered by your responder within Slack's deadline.
  - name: Telegram as a channel
    status: built
    note: Agents hold conversations with people in Telegram.
  - name: Credential store for provider secrets
    status: partial
    note: Writing, rotating, revoking and destroying secrets are recorded as events, and the running gateway refreshes from them.
  - name: Actions through project connections
    status: designed
    note: Agents call provider APIs through a proxy that holds the credential, so the agent never sees it.
  - name: Slack and Discord as channels
    status: designed
    note: Their events already arrive. Holding conversations in them is designed.
---

<ProductHeader />

A connector lets agents hear what happens in a tool your team already uses, act
in it, or talk with people through it. The gateway is the front door: it checks
every request, gives each one a stable address and hands it to any agent that
subscribes.

::: info Events work today. Actions and most channels are designed.
Every connector on this page already delivers events. Telegram also works as a
channel. Calling provider APIs and chatting in Slack or Discord are designed.
[Where it stands](#where-it-stands) has the detail.
:::

## From webhook to agent

The provider calls one URL. The gateway checks it, publishes it to a subject
that names the account and the event, and keeps it in a stream so a busy or
restarted agent misses nothing.

<ConnectorIngress />

## Every provider proves itself differently

Each provider signs its requests in its own way. The gateway knows each one, and
a request that fails its check is refused before any agent can see it.

<ConnectorVerification />

## Hear, act, talk

A connector can let agents hear what happens, act in the tool, or talk with
people through it. Each card says what works today.

<ConnectorCapabilities />

## Browse connectors

<ConnectorGallery />

## What we promise

<FeatureGrid :items="$frontmatter.guarantees" />

## Where it stands

<BuildProgress :items="$frontmatter.progress" />

## Add a connector to this page

Create `docs/product/connectors/<slug>.md` with a `connector` block in its frontmatter
and `<ConnectorHeader />` at the top of its body. The page appears in the
gallery and the sidebar on the next build.

```yaml
---
title: GitHub
connector:
  name: GitHub
  summary: React to pull requests, issues and workflow runs
  category: Developer tools
  icon: github # a https://simpleicons.org slug; omit it and set color for a monogram tile
  capabilities:
    events: available # available, preview or planned
    actions: planned
---
```

The build fails on an unknown category, icon slug or capability, so a typo
never ships a broken card. To build a connector rather than list one, see
[Add a Connector Provider](/how-to/add-a-connector-provider).

## Learn more

**Understand the design**

- [Third-Party Connections](/architecture/third-party-connections)
- [Multi-Channel Agent Routing](/architecture/multi-channel-agent-routing)
- [Keys & Secrets](/product/keys-and-secrets)

**Build and operate**

- [Add a Connector Provider](/how-to/add-a-connector-provider)
- [Integrate Connector HTTP Use](/how-to/integrate-connector-http)
- [Operate Connector Recovery and Erasure](/how-to/operate-connectors)
