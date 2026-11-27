# Sandbox in Hono

A Hono backend for the patterns in the [Hono guide](../sandbox-ws-client/docs/hono.md): run code in a sandbox per request, stream a command's output, mint a short-lived token for a browser, check capacity, and release stale leases.

## What it shows

| Route                       | Description                                                                                                                                                                                    |
| --------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `GET /health`               | Reports whether the server is in memory or gateway mode.                                                                                                                                       |
| `POST /run`                 | `{ "script": "..." }` returns `{ exitCode, stdout, stderr }`. A non-zero exit code is a result, not an error.                                                                                  |
| `POST /run/stream`          | Same body, but the response is newline-delimited JSON events (`output`, then `exit` or `error`) written while the command runs.                                                                |
| `POST /api/sandbox-session` | Calls the gateway's `POST /v1/sessions` with the API key and returns only the token.                                                                                                           |
| `GET /capacity`             | Reads `client.limits()` and says whether a lease would likely be granted. Advisory only.                                                                                                       |
| `POST /admin/reap`          | Releases this SDK's leases older than 15 minutes through `Sandbox.cleanup` (which wraps `reapLeases`). Needs `Authorization: Bearer $ADMIN_TOKEN`; accepts `?dryRun=1` and `?maxAgeSeconds=N`. |

- `server/run.ts` leases a sandbox per call and deletes it in a `finally`. A `SandboxWsError` thrown anywhere reaches `app.onError`, which answers 503 for a retryable error and 500 otherwise.
- `/run/stream` starts the command detached and follows its durable logs. A broker without durable logs answers `unsupported`, and the route then replays the buffered output once the command finishes, so the in-memory fake still works.
- `app.ts` exports the Hono app and `index.ts` serves it with `@hono/node-server`. Hono also runs on Bun, Deno and edge runtimes, where `app.fetch` is the entry point.
- `server/sandbox.ts` keeps one connection per server process on `globalThis`.

## Run it against a real stack

Needs Node 22.7 or newer and a reachable `trogon-sandbox-ws-gateway` with a worker that runs commands (`local_docker` today). Preload `alpine:latest` on that worker's Docker daemon. From `tsworkspace/`, install the workspace and build the client this example links, then start it from this directory:

```bash
docker pull alpine:latest
pnpm install
pnpm --filter @trogonai/sandbox-ws-client build
cp .env.example .env   # fill in the values
pnpm dev
```

```bash
curl -N -X POST localhost:3000/run/stream -d '{"script":"echo one; sleep 1; echo two"}'
```

| Variable                   | Description                                                                                                               |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `SANDBOX_GATEWAY_HTTP_URL` | HTTP URL of `trogon-sandbox-ws-gateway`.                                                                                  |
| `SANDBOX_GATEWAY_WS_URL`   | WebSocket URL of `trogon-sandbox-ws-gateway`.                                                                             |
| `SANDBOX_API_KEY`          | API key for the tenant. Server only.                                                                                      |
| `SANDBOX_TENANT`           | Tenant the sandboxes are leased under.                                                                                    |
| `ADMIN_TOKEN`              | Bearer token the reaper route requires.                                                                                   |
| `SANDBOX_PROVIDER`         | `local_docker` (default) or `memory` for the in-process fake. Unsupported or empty values are rejected before connecting. |
| `PORT`                     | Port for the Node server. Defaults to 3000.                                                                               |

The routes run whatever script they are given. A real app authenticates the caller and validates the script first.

## Run it without a gateway or a key

```bash
SANDBOX_PROVIDER=memory pnpm dev
```

Memory mode does not run processes, so a script is acknowledged and not executed (`exit N` sets the exit code), the streamed output arrives as one replayed chunk, and the token route answers 501 because there is no gateway.

## Smoke test

Request and result schemas use Zod. The run routes reject malformed JSON and blank scripts before leasing a sandbox. Reaper requests accept a non-negative `maxAgeSeconds` and `dryRun=0` or `dryRun=1`; empty ages and unrecognized dry-run values return 400. Browser-session replies are validated before their short-lived token is returned.

```bash
pnpm smoke
```

Calls every route through `app.request()` with the memory client. It checks the output and exit codes, that each run and stream releases its lease, the NDJSON events, input validation, the 501 from the token route, capacity, and the reaper: unauthorized calls are refused, a fresh lease is kept, a dry run releases nothing, and an aged lease is released.

## Verify a real gateway

Load the gateway URLs, tenant and API key from your local environment, then run:

```bash
pnpm test:gateway
```

The gateway lane requires `local_docker` and fails on missing configuration. It mints a browser token, opens a separate `connectWithToken` client and runs a guest command. Every lease observed from the flow is cleaned up. Tokens and API keys are never printed.

## Not covered by the memory smoke

- Live chunk-by-chunk streaming needs a broker with durable command logs; the smoke test only exercises the replay fallback.
- The gateway lane exercises a live `/v1/sessions` mint and proves incremental log delivery by observing an output marker before releasing a blocked guest command.
