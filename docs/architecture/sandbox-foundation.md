# Sandbox foundation

`@trogonai/sandbox-ws-client` is the TypeScript client for a compatible sandbox
WebSocket gateway. It provides Zod request and response schemas, a low-level
`SandboxWsClient`, a higher-level `Sandbox` handle and an in-memory client
for application tests.

The package connects to an operator-supplied gateway. It does not start a
local runtime or provide the gateway, broker or provider fleet.

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
Compatible sandbox gateway                 deployment prerequisite
    |
    v
Broker and provider workers                 deployment prerequisite
  tenant admission, quotas, capabilities,
  native ownership and lifecycle
    |
    v
Sandbox runtime
```

A complete deployment can route leases to different runtime providers. The
client carries the selected provider preferences and capability requirements;
it does not make their guarantees interchangeable.

The source package is at `tsworkspace/packages/sandbox-ws-client`. Its README
documents connection options, the supported API and examples. It is currently
consumed as a monorepo workspace package; this import does not publish a
registry release.

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

## Next runtime boundaries

The Rust foundation includes standalone building blocks that do not start a
sandbox deployment:

- `sandbox-command-supervisor` owns a Linux command family, including held
  launch, signals, output and confirmed descendant cleanup. Its caller still
  supplies container or VM isolation and authorization.
- `trogon-sandbox-contract` defines validated worker, dispatch and durable
  operation identifiers, operation kinds and legal operation-state transitions.
  The contract describes which transitions are allowed; it does not persist
  operations, perform provider effects or prove their completion.

See the [command supervisor](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/platform/sandbox-command-supervisor/README.md)
and [operation contract](https://github.com/TrogonStack/trogonai/blob/main/rsworkspace/crates/sandbox/trogon-sandbox-contract/README.md)
package documentation for their public interfaces.

A server-side import must integrate with the repository's existing NATS,
telemetry, identity and credential boundaries. It must retain exact native
ownership, distinguish acknowledgements from completion, and earn each
advertised provider capability through the intended runtime.

See [crate boundaries](../adr/0002-rust-crate-boundaries.md) and
[domain grouping](../adr/0034-rust-crate-domain-grouping.md) for the Rust
integration rules. A runtime or provider will have its own acceptance contract;
the client package alone does not supply one.
