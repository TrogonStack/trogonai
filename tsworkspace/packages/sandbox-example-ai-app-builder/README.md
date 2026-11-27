# AI app builder

Describe a small web app in a Next.js chat. The agent writes the files inside a sandbox, starts a managed Node server, checks it with Node's built-in `fetch` and shows a signed preview beside the chat.

## Run with a gateway

Use Node 22.7 or newer, pnpm and a reachable `trogon-sandbox-ws-gateway` with a `local_docker` worker. Preload the Node image on that worker's Docker daemon. From `tsworkspace/`, install the workspace and build the linked packages:

```bash
docker pull node:22-bookworm-slim
pnpm install
pnpm --filter @trogonai/sandbox-ws-client build
pnpm --filter @trogonai/sandbox-agent-tools build
```

Then configure and start the server from this directory:

```bash
cp .env.example .env.local
pnpm dev
```

Fill in the environment file before starting. Keep both API keys on the server.

| Variable                         | Description                                                                                                       |
| -------------------------------- | ----------------------------------------------------------------------------------------------------------------- |
| `SANDBOX_GATEWAY_HTTP_URL`       | HTTP URL of the gateway.                                                                                          |
| `SANDBOX_GATEWAY_WS_URL`         | WebSocket URL of the gateway.                                                                                     |
| `SANDBOX_API_KEY`                | Tenant API key.                                                                                                   |
| `SANDBOX_TENANT`                 | Tenant that owns the sandboxes.                                                                                   |
| `SANDBOX_EXAMPLE_MODEL`          | Required. A model id served by the endpoint below, or `scripted` for a fixed, keyless build. There is no default. |
| `SANDBOX_EXAMPLE_MODEL_BASE_URL` | Base URL of an OpenAI-compatible API. Required unless the model is `scripted`.                                    |
| `SANDBOX_EXAMPLE_MODEL_API_KEY`  | Bearer key for that endpoint, if it needs one. Unnecessary with `scripted`.                                       |
| `SANDBOX_PROVIDER`               | `local_docker` by default; `memory` for a file-only demo. Empty and unsupported values fail before leasing.       |

The model must support tool calls. Access and rate limits depend on the endpoint you configure.

The Docker flow uses `node:22-bookworm-slim`. Files live in `/tmp/workspace`, which is writable under the worker's non-root defaults. The image supplies Node and bash; the example does not require curl, wget or a package manager inside the guest.

The preview URL must be reachable from the browser, and the gateway must enable preview access control and allow framing. The browser receives the signed URL, never the tenant API key.

## Run a file-only demo

```bash
SANDBOX_PROVIDER=memory SANDBOX_EXAMPLE_MODEL=scripted pnpm dev
```

The scripted model writes a page and a Node server file. Memory mode stores those files in a virtual filesystem. It exposes file tools and reports that no server is running. Its preview pane shows an explanation, and files vanish when the application server restarts.

## How session and server reuse work

`server/sandbox.ts` names the persistent lease `app-builder-<session>` and caches one sandbox and tool bundle per browser session. Later chat requests reuse the bundle, including its managed process registrations. A fresh application process reattaches to the same named lease; an already-running guest server stays available.

`get_preview` checks the actual guest HTTP service with Node before returning a signed URL. A refused connection is `not_ready`; an uncertain probe or gateway routing failure is `unavailable`. The scripted model starts a managed server only for `not_ready`, reuses a ready server and reports unavailable previews without launching a duplicate. `start_server` uses port readiness, then `run_command` performs a Node HTTP check.

Chat messages, selected tool inputs and outputs, and preview states use Zod contracts. Invalid history is rejected before a sandbox is created. SDK tool results are text; `get_preview` returns its structured preview state.

The session cookie grants access to its sandbox. Add authentication and authorization before exposing this example to users. A named lease reattach does not itself reconstruct executable restart intent after a snapshot or a stopped sandbox; use the SDK's host-owned server-state receipt workflow for that lifecycle.

## Verify the application

```bash
pnpm test
pnpm smoke
pnpm test:docker
pnpm typecheck
pnpm build
```

The memory smoke drives the chat and preview handlers, checks file writes and session reuse, and requires a truthful file-only response. The Docker lane verifies that the configured workspace is writable under the worker's non-root defaults and cleans up its named probe container.

With the local gateway environment configured:

```bash
pnpm test:gateway
```

The gateway lane uses the scripted model to build and serve the app, fetches its signed preview, checks a later chat turn reuses the server, then launches a fresh Node host process to reattach and fetch it again. It cleans up every observed lease and never prints credentials. A live model response and deployment-specific browser framing still need verification in your environment.
