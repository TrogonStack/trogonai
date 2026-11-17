---
title: Agents
aside: false
pageClass: wide-page
product:
  name: Agents
  tagline: Define an agent once, revise it safely and run any revision on demand.
  order: 2
  status: preview
  icon: bot
  color: "#14B8A6"
guarantees:
  - icon: pin
    title: Running work never shifts
    text: Each session pins one exact revision. Changing an agent never changes work already in flight.
  - icon: badge-check
    title: Changes are judged first
    text: A change to an agent is a proposal. It becomes a revision only after a verifier approves it.
  - icon: shield-check
    title: Checked before it arrives
    text: Every call proves who sent it and passes policy before any agent sees it.
  - icon: scroll-text
    title: Nothing goes unrecorded
    text: Allowed and refused calls are both audited. A decision that cannot be recorded is refused.
  - icon: globe
    title: Only the doors you open
    text: An agent reaches only the exact destinations its policy allows. Everything else is refused.
  - icon: key-round
    title: Agents never hold your secrets
    text: Credentials are added on the way out, so the agent uses a connection without ever seeing the key.
  - icon: cpu
    title: Bring any runtime
    text: Claude Code, Codex or your own agent. A planner, a coder and a reviewer are one kind of thing with different settings.
progress:
  - name: Editors talk to remote agents over ACP
    status: built
    note: A stdio bridge for editors and an HTTP and WebSocket server, both over NATS.
  - name: Agent-to-agent calls through a policy gateway
    status: built
    note: A2A over NATS with caller identity, layered policy, redaction, audit and push notifications.
  - name: Tools over MCP
    status: built
    note: MCP carried over NATS for stdio and HTTP. There is no tool registry yet.
  - name: Proof of possession at the edge
    status: built
    note: Callers at the public A2A endpoint prove they hold their key on every request.
  - name: Egress control and workload identity
    status: built
    note: A proxy that admits only allowed destinations per agent, using attested workload identity.
  - name: Platform audit stream
    status: built
    note: Shared by the gateway and the egress proxy. Records that must be kept fail closed.
  - name: Agent discovery catalog
    status: partial
    note: The catalog model and HTTP routes exist with an in-memory store. NATS storage is not wired.
  - name: Policy bindings
    status: partial
    note: Contracts and an evaluator library exist. No service enforces them yet.
  - name: Agent definitions, revisions and proposals
    status: designed
    note: Protobuf contracts and the stream design. No registry service yet.
  - name: Workflows across agents
    status: designed
    note: Sequence, parallel, repeat and wait, with every step pinned to a revision.
  - name: Connections with injected credentials
    status: designed
    note: Agents use third-party accounts through a proxy that holds the credential.
  - name: Model access through the provider gateway
    status: designed
    note: The gateway serves health checks only. Routing and credential binding are designed.
---

<ProductHeader />

An agent is a stable identity with a history of exact revisions. You can reach
it from your editor, a chat app or another agent, and every call is checked and
recorded on the way in. Changing an agent never touches work already running.

::: info Talking to agents is built. Defining and running them is designed.
The protocols, the policy gateway, egress control and audit work today. The
registry for definitions and revisions, workflows and connections are designed.
[Where it stands](#where-it-stands) has the detail.
:::

## One agent, many revisions

An agent never changes in place. A change starts as a proposal, gets judged, and
only then becomes the next revision. Sessions keep the revision they started on.

<AgentRevisions />

## Reach it from anywhere

Editors, chat apps, other agents and tools all meet on one message bus, each in
the protocol it already speaks.

<AgentReach />

## Every call is checked on the way in

Before a request from another agent reaches yours, the gateway confirms who sent
it and runs it through each layer of policy. A refusal at any layer stops the
call and is recorded.

<AgentGateway />

## Fenced in by default

A running agent gets only what its policy grants: where it may connect, what it
may spend and which accounts it may use.

<AgentGuardrails />

## Work that spans agents

Larger jobs become workflows. Each step runs in a fresh session pinned to an
exact revision, so a restart picks up where the work left off instead of asking
a model to remember.

<AgentWorkflow />

## What we promise

<FeatureGrid :items="$frontmatter.guarantees" />

## Where it stands

<BuildProgress :items="$frontmatter.progress" />

## Learn more

**Understand the design**

- [Agent Platform](/architecture/agent-platform)
- [Workflows](/architecture/workflows)
- [Multi-channel Agent Routing](/architecture/multi-channel-agent-routing)

**Connecting and protecting**

- [ACP Conformance](/architecture/acp-conformance)
- [Third-party Connections](/architecture/third-party-connections)
- [Keys & Secrets](/product/keys-and-secrets)
- [Sessions](/product/sessions)
