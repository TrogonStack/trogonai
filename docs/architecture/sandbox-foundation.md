# Sandbox foundation

`@trogonai/sandbox-ws-client` is the TypeScript client for a compatible sandbox
WebSocket gateway. It provides Zod request and response schemas, a low-level
`SandboxWsClient`, a higher-level `Sandbox` handle and an in-memory client
for application tests.

The client connects to a gateway supplied by the operator. The
[Sandbox product page](../product/sandbox.md) covers the broker, gateways and
provider workers in the public source and the work underway.

## Where the client sits

```text
Application
    |
    v
Sandbox handle / SandboxWsClient
  Zod schemas, typed identifiers,
  request lifecycle and client cleanup
    |
    | authenticated gateway connection
    v
Sandbox WebSocket gateway
    |
    | NATS request/reply
    v
Sandbox broker
  tenant admission, quotas and worker selection
    |
    | worker dispatch (signing configurable)
    v
Provider worker and its provider backend
  native resources and lifecycle
```

The broker matches the lease's requirements against registered worker
capabilities. The worker executes the operation on its provider. Running this
path requires a configured gateway, NATS with JetStream, a broker and an
eligible worker. The [interactive protocol map](../product/sandbox.md#sandbox-protocols)
shows the request and reply for each client. Rust applications can also use the
[Rust client](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/sandbox/trogon-sandbox-client/README.md)
to reach the broker over NATS.

The TypeScript client is a workspace package. For connection options and
examples, see the
[package README](https://github.com/TrogonStack/trogonai/blob/main/tsworkspace/packages/sandbox-ws-client/README.md).

## API responsibilities

| Surface | Responsibility |
| --- | --- |
| `SandboxWsClient` | Gateway connection, request/response exchange, subscriptions and lower-level operations |
| `Sandbox` | Lease-scoped commands, files, processes, ownership-aware handles and cleanup helpers |
| Zod schemas and typed identifiers | Validate supported wire shapes and values before use |
| `MemorySandboxClient` | Model supported operations for application tests without a live gateway |

The package covers lease operations, commands and named processes, file
operations and transfer, network policy requests, sandbox users, snapshots,
terminals and language-server helpers. Support for an operation in the SDK
does not establish that a particular provider implements it. Consult the
gateway's advertised capabilities and the connected deployment's acceptance
evidence.

The in-memory implementation explicitly refuses features it does not model.
Its successful tests demonstrate client behavior, not operating-system
isolation, network enforcement or provider conformance.

## Ownership and completion

Client connection lifetime and sandbox lifetime are separate. Closing a
WebSocket connection is not evidence that the native sandbox has been
destroyed. Stopping a named sandbox and deleting a sandbox are also different
operations, with behavior governed by the gateway and provider.

Always release a sandbox handle through the intended cleanup operation,
including error paths. The SDK provides scoped disposal and explicit methods;
its README describes when disposal stops a named sandbox or deletes an unnamed
one.

A successful request can acknowledge desired state before native work
finishes. A stop acknowledgement is not by itself proof of native deletion,
durable retained state or absence of descendant processes. Use the connected
gateway's applicable completion contract and verify native state during
provider acceptance.

The client retries only the request classes covered by its retry contract.
Retries do not grant authority over a replacement execution or make an
ambiguous native effect safe to repeat.

## Build and verification

The SDK participates in the existing pnpm workspace and declared Bazel graph.
From `tsworkspace`, the package scripts build the emitted ESM package,
typecheck source and public API fixtures, verify documentation snippets, run
contract tests and lint the package:

```sh
pnpm --filter @trogonai/sandbox-ws-client build
pnpm --filter @trogonai/sandbox-ws-client typecheck
pnpm --filter @trogonai/sandbox-ws-client test
pnpm --filter @trogonai/sandbox-ws-client lint
```

The emitted-package smoke test imports the built package and exercises its
public lifecycle through the memory client. Consumer type fixtures also check
the generated declarations. These checks protect a different seam from
source-only TypeScript validation.

Bazel declares the SDK sources, test helpers, public type fixtures,
documentation inputs and package metadata. This keeps test discovery and
cache invalidation connected to the files the tests actually use.

Live gateway/provider acceptance requires a separately prepared deployment.
Operator scripts with external infrastructure requirements are not part of
this standalone client package's local contract test suite.

## Runtime responsibilities

- The WebSocket gateway exchanges a tenant API key for a short-lived session
  token. The browser uses that token to call the gateway, which holds the NATS
  connection. [Gateway handshake](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/sandbox/trogon-sandbox-ws-gateway/README.md#auth-handshake)
- The broker admits requests against tenant policy and quotas, selects a worker,
  signs dispatches when configured and records outcomes in JetStream. [Broker responsibilities](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/sandbox/trogon-sandbox-broker/README.md)
- The contract defines lease requirements, worker capabilities, isolation tiers
  and valid operation transitions. Workers report which operations their runtimes
  can carry out. [Sandbox contract](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/sandbox/trogon-sandbox-contract/README.md)
- On Linux, the command supervisor manages a command and its descendants through
  launch, signals, output and cleanup. Its caller supplies isolation and
  authorization. [Command supervisor](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/platform/sandbox-command-supervisor/README.md)

## Sandbox broker and workers

The broker, the workers and the operator tooling behind this client are documented in the sandbox guides. Start with [your first sandbox lease](../get-started/sandbox/first-sandbox-lease.md), or use the [sandbox documentation map](../reference/sandbox/documentation-map.md) to find a tutorial, how-to guide, explanation or reference.
