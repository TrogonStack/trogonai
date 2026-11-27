# Sandbox in Next.js

A Next.js App Router example of the patterns in the [Next.js guide](../sandbox-ws-client/docs/nextjs.md): run code in a sandbox from a Route Handler and a Server Action, mint a short-lived token for the browser, and release stale leases from a cron route.

## What it shows

- `server/run.ts` is the one module both entry points share. It leases a sandbox, runs `sh -c <script>`, and deletes the sandbox in a `finally`. A non-zero exit code is a result, not an error; a `SandboxWsError` becomes its `hint`.
- `app/api/run/route.ts` is the Route Handler: `POST { "script": "..." }` returns `{ exitCode, stdout, stderr }`, or 503 when the error is retryable. It sets `maxDuration` above the sandbox timeout.
- `app/actions.ts` is the Server Action behind the form on the home page (`app/runner.tsx`, a client component using `useActionState`).
- `app/api/sandbox-session/route.ts` calls the gateway's `POST /v1/sessions` with the API key and returns only the token, so the key never reaches the browser.
- `app/api/reap/route.ts` is the cron-style reaper. It calls `Sandbox.cleanup`, which wraps `reapLeases`, for this SDK's leases older than 15 minutes, so a run that died before its `finally` does not hold a lease until the broker's timeout. It requires `Authorization: Bearer $CRON_SECRET`, accepts `?dryRun=1` and `?maxAgeSeconds=N`, and `vercel.json` schedules it every 15 minutes.
- `server/sandbox.ts` keeps one connection per server process on `globalThis`, so `next dev` hot reloads reuse it.

## Run it against a real stack

Needs Node 22.7 or newer and a reachable `trogon-sandbox-ws-gateway` with a worker that runs commands (`local_docker` today). Preload `alpine:latest` on that worker's Docker daemon. From `tsworkspace/`, install the workspace and build the client this example links, then start it from this directory:

```bash
docker pull alpine:latest
pnpm install
pnpm --filter @trogonai/sandbox-ws-client build
cp .env.example .env.local   # fill in the values
pnpm dev
```

| Variable                   | Description                                                                                                               |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `SANDBOX_GATEWAY_HTTP_URL` | HTTP URL of `trogon-sandbox-ws-gateway`.                                                                                  |
| `SANDBOX_GATEWAY_WS_URL`   | WebSocket URL of `trogon-sandbox-ws-gateway`.                                                                             |
| `SANDBOX_API_KEY`          | API key for the tenant. Server only.                                                                                      |
| `SANDBOX_TENANT`           | Tenant the sandboxes are leased under.                                                                                    |
| `CRON_SECRET`              | Bearer token the reaper route requires.                                                                                   |
| `SANDBOX_PROVIDER`         | `local_docker` (default) or `memory` for the in-process fake. Unsupported or empty values are rejected before connecting. |

The routes run whatever script they are given. A real app authenticates the caller and validates the script first.

## Run it without a gateway or a key

```bash
SANDBOX_PROVIDER=memory pnpm dev
```

Memory mode is honest about what it is: `MemorySandboxClient` does not run processes, so a script is acknowledged and not executed (`exit N` sets the exit code), and the token route answers 501 because there is no gateway.

## Smoke test

Request and result schemas use Zod. The run route rejects malformed JSON and blank scripts before leasing, and the server action accepts text rather than coercing an uploaded file into a command. Reaper requests accept a non-negative `maxAgeSeconds` and `dryRun=0` or `dryRun=1`; empty ages and unrecognized dry-run values return 400. Browser-session replies are validated before their short-lived token is returned.

```bash
pnpm smoke
```

Calls the route handlers and the Server Action in-process with the memory client. It checks the output and exit codes, that each run releases its lease, input validation, the 501 from the token route, and the reaper: unauthorized calls are refused, a fresh lease is kept, a dry run releases nothing, and an aged lease is released. `pnpm typecheck` and `pnpm build` check the types and the Next.js build.

## Verify a real gateway

Load the gateway URLs, tenant and API key from your local environment, then run:

```bash
pnpm test:gateway
```

The gateway lane requires `local_docker` and fails on missing configuration. It mints a browser token, opens a separate `connectWithToken` client and runs a guest command. Every lease observed from the flow is cleaned up. Tokens and API keys are never printed.

## Not covered by the memory smoke

- The gateway lane exercises the run route, Server Action and browser token against a live `/v1/sessions`. The Vercel cron trigger still needs deployment verification.
- The browser-side `connectWithToken` flow from the guide is not part of this example.
