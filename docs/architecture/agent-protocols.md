# How agent protocols fit together

TrogonAI uses A2A, ACP and MCP for different interactions. Choosing a protocol
starts with who is talking and what they need to do.

| Interaction | Protocol | What the conversation carries |
| --- | --- | --- |
| An agent asks another agent to do work | A2A, Agent2Agent | Messages, tasks, progress, artifacts and cancellation |
| An editor or another client works with an agent | ACP, Agent Client Protocol | Prompts, session updates, permission requests, file and terminal callbacks |
| An AI application uses tools and context | MCP, Model Context Protocol | Tool calls, resources, prompts and negotiated server/client capabilities |

These roles can coexist in one application. A client can use ACP to prompt an
agent that delegates through A2A and calls an MCP tool while doing the work.
Each conversation keeps its own lifecycle and capabilities.

## Protocol, transport and backbone

A protocol defines messages and their meaning. A transport delivers those
messages. Stdio is useful when a client launches a local bridge; HTTP and
WebSocket provide remote connection shapes. NATS is the internal backbone that
routes messages between the bridge and a service.

<ProtocolMap />

The map shows each protocol's logical participants. Transport adapters carry
their separate conversations over NATS. An MCP tool call needs an MCP server,
and an ACP prompt needs an ACP agent implementation. The agent application owns
any decision to delegate work or call a tool.

[ADR#0003](../adr/0003-ai-protocol-transport-taxonomy.md) defines this vocabulary.
[ADR#0004](../adr/0004-protocol-and-transport-layering.md) keeps transport and
protocol types at their boundaries so domain logic can use its own commands and
values.

## A2A delegates work

A2A exposes an agent as a peer that receives messages and manages tasks. A task
can outlive an initial response, report updates, return artifacts and accept
cancellation. Agent cards describe the capabilities a caller can discover.

TrogonAI has NATS client/server adapters, a local HTTP adapter and stdio support.
The policy gateway and the cross-binding edge, `a2a-bridge`, are separate
components. The gateway checks calls routed through its ingress path. The local
HTTP adapter sends calls directly to agents unless gateway routing is enabled.
The bridge obtains caller-scoped NATS credentials before forwarding. Its default
`enforce` mode requires verified caller proof. The development-only `shadow`
mode can admit a failed proof, while `off` skips proof verification.

The bridge listener serves plain HTTP. Public HTTPS requires an external
TLS-terminating reverse proxy or ingress in front of it. This repository does
not define a TLS-terminating deployment for the bridge; the deployment owner
must configure that boundary before exposing it beyond loopback. Deploying the
local HTTP adapter does not implicitly deploy these controls.

## ACP connects a client to an agent

ACP gives an editor or another client an interactive connection to an agent.
The client sends prompts and receives updates. The agent can ask the client for
permission or for file and terminal operations, subject to their negotiated
capabilities.

The stdio and remote bridges carry this conversation to NATS-backed agent
implementations. ACP sessions here are protocol sessions. Their existence does
not establish implementation of the platform's proposed
[durable session aggregate](./session-aggregate.md).

The [ACP conformance matrix](./acp-conformance.md) records method-level support,
SDK pins, typed payload limits and remote transport differences. In particular,
native MCP messages embedded in ACP need a request-lifetime host and routing
that the bridge does not currently provide.

## MCP supplies tools and context

MCP connects an AI application to a server exposing tools, resources and
prompts. The server declares its capabilities; the client and server may also
negotiate callbacks such as sampling and elicitation.

TrogonAI carries MCP over NATS and provides stdio and Streamable HTTP bridges.
A backend MCP server still implements the tools and advertises the capabilities
it supports. Transport connectivity does not create a tool registry or grant
permission to use a tool.

An MCP transport is also separate from
[third-party connection custody](./third-party-connections.md). Connecting to an
external OAuth-protected MCP server requires the credential and authorization
work described in the [MCP authorization reference](../reference/connector-mcp-authorization.md).

## Carrying traffic and admitting a caller

Routing a request over NATS does not establish who the external caller is or
what they may do. A2A has an authenticated edge and gateway enforcement path.
MCP and ACP remote listeners currently share service-level NATS connections.
The MCP listener configures Host validation, but does not configure Origin
validation. The ACP listener also enforces Origin. These checks do not
authenticate individual callers.

[ADR#0087](../adr/0087-mcp-acp-caller-identity-from-subject-and-edge-credential.md)
proposes the missing MCP/ACP identity boundary and remains draft. Apply security
claims to the entry point that implements them, rather than to every protocol
bridge on the backbone.

## Related documentation

- [Protocol support reference](../reference/agent-protocols.md) for implemented surfaces and gaps.
- [Protocols product overview](../product/protocols.md) for the website showcase.
- [ACP conformance](./acp-conformance.md) for detailed compatibility evidence.
- [Agent platform](./agent-platform.md) for definitions, revisions and execution ownership.
