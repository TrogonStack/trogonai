# Run the whole sandbox stack locally with one command

`trogon-sandbox-dev` starts `nats-server`, the broker, a worker and the WebSocket gateway as one process group, registers an API key, and prints the URLs and the private key file path a client needs. Ctrl-C stops everything. If any process dies, the rest are stopped too.

## When to use this

- Developing against the TypeScript or Rust client without starting four processes by hand.
- A throwaway local environment for demos and manual checks.

For unit tests of code that uses the client, do not start any of this. Use the in-memory client described below.

## Prerequisites

- `nats-server` on PATH, for example from `brew install nats-server`. `trogon-sandbox-dev` exits with an install hint when it is missing.
- Docker, only if you want the Docker worker.

## Start the stack

From `rsworkspace/`, build the binaries `trogon-sandbox-dev` launches. It looks for them in `--bin-dir`, then next to its own executable, then on PATH:

```sh
cargo build -p trogon-sandbox-dev -p trogon-sandbox-broker -p trogon-sandbox-ws-gateway \
  -p trogon-sandbox-docker-worker -p trogon-sandbox-memory-worker
cargo run -p trogon-sandbox-dev
```

The output looks like this:

```text
trogon-sandbox-dev is up (worker: Docker)
  NATS URL:      nats://127.0.0.1:4222
  gateway HTTP:  http://127.0.0.1:8443
  gateway WS:    ws://127.0.0.1:8443
  tenant:        dev
  API key file:  /path/to/private/temporary/api-key
```

The generated credential stays in that file while the stack runs. The default temporary file is removed when the command exits. To choose the output file, pass `--api-key-file /path/to/dev-api-key` or set `SANDBOX_DEV_API_KEY_FILE`. An existing file or symlink is refused before the stack starts, and a file you selected remains after shutdown. On Unix the file is created with mode `0600`.

To reuse a credential, supply it through `SANDBOX_DEV_API_KEY`. Neither the startup output nor `--help` prints the value.

The command starts the broker with `SANDBOX_INSECURE_DEV=true`. By default the broker enforces bearer authentication, the image allow-list and the worker registry, which needs SpiceDB. The insecure mode switches those off, so use it only on a local stack. It also gives the broker a throwaway dispatch signing key and requires workers to verify it, as described in [Sign lease dispatches and verify them on workers](./sign-worker-dispatches.md).

## Choose the worker

`--worker auto`, the default, picks in this order:

1. `docker` when the Docker daemon answers.
2. `subprocess` on Linux or macOS when `--agent-binary` is given. Add `--isolation-backend none` where the platform backend is not available.
3. `memory`, the map-backed worker with scripted commands and no containers.

Pass `--worker docker`, `--worker subprocess` or `--worker memory` to force one. A forced worker whose binary is missing fails with an error naming the binary to build.

## Flags

| Flag | Default | Purpose |
| --- | --- | --- |
| `--nats-port` (`SANDBOX_DEV_NATS_PORT`) | `4222` | Client port of the `nats-server` it starts, with JetStream on. |
| `--gateway-listen` (`SANDBOX_DEV_GATEWAY_LISTEN`) | `127.0.0.1:8443` | Gateway bind address. |
| `--worker` (`SANDBOX_DEV_WORKER`) | `auto` | `auto`, `docker`, `subprocess` or `memory`. |
| `--image` (`SANDBOX_IMAGE`) | `alpine:3.20` | Default image of the Docker worker. |
| `--agent-binary` (`SANDBOX_AGENT_BINARY`) | none | In-sandbox agent binary, required by the subprocess worker. |
| `--isolation-backend` (`SANDBOX_ISOLATION_BACKEND`) | worker default | Isolation backend of the subprocess worker, for example `none`. |
| `--bin-dir` (`SANDBOX_DEV_BIN_DIR`) | none | Directory holding the sandbox binaries. |
| `--api-key` (`SANDBOX_DEV_API_KEY`) | random | Fixed key. Prefer the variable, to keep it out of command arguments. |
| `--api-key-file` (`SANDBOX_DEV_API_KEY_FILE`) | temporary file | Write the credential to a new file. |
| `--tenant` | `dev` | Tenant the key belongs to. |
| `--store-dir` | temporary directory | Keep JetStream data between runs. |

The broker runs with in-memory quota and idempotency and one JetStream replica, which is only appropriate for local use.

## Point the TypeScript client at it

Load the printed key file into `SANDBOX_API_KEY` in the environment of the client process. Keep that file private and avoid printing its contents.

This example uses the Docker worker. When you run the subprocess worker instead, name its provider in `providerPref`.

```ts
import { Sandbox, SandboxWsClient, tenantId } from '@trogonai/sandbox-ws-client';

const client = await SandboxWsClient.connect({
  gatewayHttpUrl: 'http://127.0.0.1:8443',
  gatewayWsUrl: 'ws://127.0.0.1:8443',
  apiKey: process.env.SANDBOX_API_KEY!,
});
const sandbox = await Sandbox.create(client, {
  tenant: tenantId('dev'), providerPref: ['local_docker'], timeoutMs: 60_000,
});
```

The client package is described in [Sandbox foundation](../../architecture/sandbox-foundation.md).

## Unit tests need no stack

`MemorySandboxClient` from `@trogonai/sandbox-ws-client` is the zero-infrastructure path. It implements the same client interface with a virtual filesystem and scripted commands, so code written against `Sandbox.create(client, ...)` runs in a plain test process:

```ts
import { MemorySandboxClient, Sandbox, tenantId } from '@trogonai/sandbox-ws-client';

const client = new MemorySandboxClient({
  commands: { greet: () => ({ exitCode: 0, stdout: 'hi\n' }) },
});
const sandbox = await Sandbox.create(client, { tenant: tenantId('t'), timeoutMs: 1000 });
```

Verbs it does not model throw `SandboxWsError` with code `unsupported`. Use `--worker memory` when you need the same behaviour over the real wire.

## Verify

On Unix, `cargo test -p trogon-sandbox-dev` starts the stack with the memory worker and checks private key file permissions, generated and supplied credential authentication, startup and help output, and refusal of existing files and symlinks. It needs `nats-server` and the binaries above built.
