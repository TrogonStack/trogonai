# Glossary

Shared vocabulary for this repository, grouped by concern. Each term has its own page, and its definition links to the ADR or architecture document that owns the decision behind it.

Link to a term from any other document with a relative path, for example `[decider](../glossary/decider)`.

## Repository and process

- [ADR](./adr)
- [Workspace](./workspace)
- [Crate](./crate)

## Protocols and transports

- [Protocol](./protocol)
- [Transport](./transport)
- [ACP](./acp)
- [MCP](./mcp)
- [A2A](./a2a)
- [ARD](./ard)
- [Bridge](./bridge)
- [JSON-RPC](./json-rpc)
- [ConnectRPC](./connectrpc)

## Event sourcing and the decider

The full narrative lives in [Decider Platform](../architecture/decider.md); these
entries are the quick reference.

- [Event sourcing](./event-sourcing)
- [Decider](./decider)
- [decide / evolve / initial_state](./decide-evolve-initial-state)
- [Command](./command)
- [Event](./event)
- [Decision](./decision)
- [Event envelope](./event-envelope)
- [Headers](./headers)
- [Stream](./stream)
- [Write precondition](./write-precondition)
- [Snapshot](./snapshot)
- [Checkpoint](./checkpoint)
- [Projection](./projection)
- [Processor](./processor)
- [Tenant](./tenant)
- [Admission control](./admission-control)
- [Retention watermark](./retention-watermark)

## Agent execution model

How an agent is defined, versioned, and run. The full narrative is split across
[ADR#0025](../adr/0025-agent-definition-data-ownership.md) (definition ownership),
[ADR#0031](../adr/0031-agent-implementation-and-session-plan.md) (implementation
and session plan), and
[ADR#0032](../adr/0032-model-route-and-credential-binding.md) (model routing and
credentials). These are draft decisions; treat the entries as the intended model,
not current behavior.

- [Agent](./agent)
- [AgentConfiguration](./agentconfiguration)
- [AgentRevision](./agentrevision)
- [AgentImplementation](./agentimplementation)
- [ModelSelection](./modelselection)
- [Session](./session)
- [SessionExecutionPlan](./sessionexecutionplan)
- [ExecutionAttempt](./executionattempt)
- [External delegated agent](./external-delegated-agent)
- [ResolvedModelRoute](./resolvedmodelroute)
- [ModelProviderConnection](./modelproviderconnection)
- [CredentialBinding](./credentialbinding)
- [ModelAccessGrant](./modelaccessgrant)
- [Model access service](./model-access-service)

## Messaging and storage infrastructure

- [NATS](./nats)
- [JetStream](./jetstream)
- [NATS micro](./nats-micro)
- [KV bucket](./kv-bucket)

## WebAssembly execution

- [WASM](./wasm)
- [Component](./component)
- [WIT](./wit)
- [wasmtime](./wasmtime)
- [Fuel](./fuel)
- [Epoch](./epoch)

## Wire contracts and serialization

- [Protocol Buffers](./protocol-buffers)
- [Type URL](./type-url)

## Identity, security, and multi-tenancy

- [AAuth](./aauth)
- [PoP](./pop)
- [OpenBao](./openbao)
- [Managed key](./managed-key)
- [Customer managed key](./customer-managed-key)

## Observability

- [OpenTelemetry](./opentelemetry)
- [semconv](./semconv)
