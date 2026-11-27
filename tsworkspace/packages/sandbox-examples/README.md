# Sandbox Examples

Runnable TypeScript examples for [`@trogonai/sandbox-ws-client`](../sandbox-ws-client). Each example lives in its own directory under `examples/` with a README describing what it shows.

## Examples

| Example                                             | Description                                                                                     |
| --------------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| [hello-world](examples/hello-world)                 | Create a sandbox, run a command, print the output, delete the sandbox.                          |
| [process-execution](examples/process-execution)     | Run code and shell commands in an isolated sandbox and check exit codes.                        |
| [file-system](examples/file-system)                 | List, create, write, search, edit, inspect, move, and delete files.                             |
| [git](examples/git)                                 | Clone a repository into a sandbox and pull the latest changes.                                  |
| [lsp](examples/lsp)                                 | Run a language server in a sandbox for document symbols and completions.                        |
| [multi-provider](examples/multi-provider)           | Run the same code on an in-process fake or any provider kind, branching on reported features.   |
| [coding-agent](examples/coding-agent)               | Clone a repo into a sandbox, let an AI SDK tool loop edit it, and print the diff.               |
| [ai-app-builder](../sandbox-example-ai-app-builder) | Next.js chat where an agent builds a web app in a sandbox and shows a live preview.             |
| [nextjs](../sandbox-example-nextjs)                 | Next.js route handler, server action, browser token route and a cron reaper over `reapLeases`.  |
| [hono](../sandbox-example-hono)                     | Hono routes that run code, stream command output, mint a token, check capacity and reap leases. |

## Prerequisites

- Node.js 22.7 or newer and pnpm. Use `run-typescript.mjs` to adapt the native TypeScript flags to your Node version.
- A reachable `trogon-sandbox-ws-gateway` and an API key for your tenant.
- A worker that supports the selected example's commands and image. Check its reported capabilities and the example README for additional requirements such as interactive stdin; provider names alone do not establish those features.

## Images

`Sandbox.create` defaults to `alpine:latest`, which has no `git` or compiler. Coding workloads should pass an image explicitly. The image must ship the tools the workload needs, such as `git` and a compiler. The examples use stock public images (`buildpack-deps:bookworm-scm`, `node:22-bookworm-slim`) so they run without a build step.

## Setup

From `tsworkspace/`, install the workspace and build the packages the examples link:

```bash
pnpm install
pnpm --filter @trogonai/sandbox-ws-client build
pnpm --filter @trogonai/sandbox-agent-tools build
```

Export the connection settings every example reads:

| Variable                   | Description                                   |
| -------------------------- | --------------------------------------------- |
| `SANDBOX_GATEWAY_HTTP_URL` | HTTP URL of `trogon-sandbox-ws-gateway`.      |
| `SANDBOX_GATEWAY_WS_URL`   | WebSocket URL of `trogon-sandbox-ws-gateway`. |
| `SANDBOX_API_KEY`          | API key for the tenant.                       |
| `SANDBOX_TENANT`           | Tenant the sandboxes are leased under.        |

Then run an example from this directory, for example `node run-typescript.mjs examples/hello-world/index.ts`. The [coding-agent](examples/coding-agent) also has a `pnpm coding-agent` entry point and rejects providers incompatible with its supplied image.

`pnpm smoke` runs [multi-provider](examples/multi-provider) and [coding-agent](examples/coding-agent) in keyless in-memory mode, with no gateway or API key.
