---
title: Protocols
aside: false
pageClass: wide-page
product:
  name: Protocols
  tagline: Connect agents, editors and tools in the protocols they already speak.
  order: 6
  status: preview
  icon: network
  color: "#6366F1"
protocols:
  - name: A2A
    fullName: Agent2Agent
    icon: users
    from: Agent
    to: Agent
    purpose: Send work to another agent and follow its task updates.
    transport: HTTP, stdio and NATS
    link: "#a2a-agent-to-agent-work"
  - name: ACP
    fullName: Agent Client Protocol
    icon: terminal
    from: Editor or client
    to: Agent
    purpose: Prompt an agent, review its work and answer permission requests.
    transport: Stdio, HTTP, WebSocket and NATS
    link: "#acp-an-agent-in-your-editor"
  - name: MCP
    fullName: Model Context Protocol
    icon: plug
    from: AI application
    to: Tools and context
    purpose: Discover and call tools, read resources and retrieve prompts.
    transport: Stdio, Streamable HTTP and NATS
    link: "#mcp-tools-and-context"
features:
  - icon: route
    title: Pick the right conversation
    text: A2A carries agent work, ACP connects a client to an agent and MCP exposes tools and context.
  - icon: network
    title: Keep services connected
    text: Protocol adapters carry traffic over the NATS backbone while clients use a local or remote transport.
  - icon: layers
    title: Keep responsibilities clear
    text: A bridge carries messages. Agent execution, tool implementation and authorization have their own owners.
progress:
  - name: A2A messaging and task updates
    status: built
    note: NATS client and server adapters, HTTP and stdio surfaces, task streaming and push notification support.
  - name: A2A policy and authenticated edge
    status: built
    note: Gateway enforcement and a separate HTTPS bridge with caller proof of possession and caller-scoped NATS credentials.
  - name: ACP editor and remote connections
    status: built
    note: A stdio bridge and an official SDK-backed HTTP and WebSocket listener connect clients to NATS-backed agents.
  - name: MCP tools, resources and prompts
    status: built
    note: NATS transports for both protocol roles, plus stdio and Streamable HTTP bridges.
  - name: Verified callers at MCP and ACP remote edges
    status: designed
    note: The caller-identity proposal remains draft. Host and Origin checks do not authenticate the caller.
---

<ProductHeader />

An editor talking to an agent, an agent delegating work and an agent calling a
tool are different conversations. TrogonAI provides adapters for each, with NATS
carrying messages between services.

::: info Adapters are built; support has explicit limits
The protocol bridges exist in the repository. Supported methods, security
boundaries and remaining integration gaps are documented in the
[protocol support reference](/reference/agent-protocols). A bridge does not
supply the agent or tool service behind it.
:::

## The right protocol for the job

Follow an agent's conversations with its client, another agent and its tools.
Select a protocol to see what travels in each direction.

<ProtocolMap />

<ProtocolOverview :items="$frontmatter.protocols" />

## A2A: agent-to-agent work

Use A2A when another agent should take on work and report what happens. The
adapters carry messages, task queries, cancellation and streamed updates. Agent
cards describe an agent's capabilities; push notifications provide another way
to follow its work.

The policy gateway checks incoming agent calls. A separate HTTPS bridge verifies
caller proof of possession before obtaining caller-scoped NATS credentials.
The local HTTP adapter and the authenticated HTTPS bridge are distinct entry
points, so choose the boundary appropriate to your deployment.

[A2A support and boundaries](/reference/agent-protocols#a2a)

## ACP: an agent in your editor

Use ACP when an editor or another client needs an interactive agent. Prompts,
updates, file access, terminal callbacks and permission requests cross the
bridge while the agent runs behind NATS.

Local clients can launch the stdio bridge. Remote clients can use the HTTP and
WebSocket listener. Support follows the pinned SDK and its schema, with known
gaps tracked in the [ACP conformance matrix](/architecture/acp-conformance).
Native MCP-over-ACP messages and connection-scoped authentication push updates
are not routed today.

[ACP support and boundaries](/reference/agent-protocols#acp)

## MCP: tools and context

Use MCP when an AI application needs to call a tool, read a resource or retrieve
a prompt. The NATS adapter supports the client and server roles; stdio and
Streamable HTTP bridges connect existing clients to a backend server.

Tool behavior and available capabilities come from that backend. The transport
does not create a managed tool registry, authorize individual tools or provide
OAuth custody. The remote MCP and ACP listeners still need the caller-identity
work described in their draft proposal before they can serve that security role.

[MCP support and boundaries](/reference/agent-protocols#mcp)

## How the pieces fit

<FeatureGrid :items="$frontmatter.features" />

## Where it stands

<BuildProgress :items="$frontmatter.progress" />

## Learn more

- [How agent protocols fit together](/architecture/agent-protocols)
- [Protocol support reference](/reference/agent-protocols)
- [ACP conformance](/architecture/acp-conformance)
- [Agents](/product/agents)
- [Third-party connections](/architecture/third-party-connections)
