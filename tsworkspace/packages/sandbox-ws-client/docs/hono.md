# Use the sandbox client from Hono

Use `@trogonai/sandbox-ws-client` to run code for a Hono request and mint a short-lived token for a browser. Hono supplies the routes; the sandbox lifecycle stays in plain client code.

The runnable integration uses Node 22.7 or newer and pnpm. Its native TypeScript launcher supplies the transform flag on Node versions that support it and uses native TypeScript on newer versions.

For the runnable Docker example, preload `alpine:latest` on the worker's Docker daemon before creating leases. The framework examples use the same explicit `local_docker` or `memory` configuration as their package READMEs.

## Keep the API key on the server

`SANDBOX_API_KEY` is the tenant's full credential. Read it from the environment of the Hono process and never send it to a page. Browsers get a session token instead (see below).

## Run code for a request

Open one client per request unless you have measured that sharing is worth it, and release the sandbox in a `finally` so a failed request does not leave a lease running until its timeout.

```ts
import { Sandbox, SandboxWsClient, tenantId } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

export const scriptSchema = z.string().refine((script) => script.trim().length > 0, 'script must not be blank');
const resultSchema = z.object({ exitCode: z.number().int(), stdout: z.string(), stderr: z.string() });
export type RunResult = z.infer<typeof resultSchema>;

export async function runShell(script: string): Promise<RunResult> {
  const command = scriptSchema.parse(script);
  const client = await SandboxWsClient.connect({
    gatewayHttpUrl: process.env.SANDBOX_GATEWAY_HTTP_URL!,
    gatewayWsUrl: process.env.SANDBOX_GATEWAY_WS_URL!,
    apiKey: process.env.SANDBOX_API_KEY!,
  });
  try {
    const sandbox = await Sandbox.create(client, {
      tenant: tenantId(process.env.SANDBOX_TENANT!),
      providerPref: ['local_docker'],
      timeoutMs: 60_000,
      networkPolicy: 'deny-all',
    });
    try {
      const result = await sandbox.runCommand({ cmd: 'sh', args: ['-c', command], timeoutSeconds: 30 });
      return resultSchema.parse({ exitCode: result.exitCode, stdout: result.stdout(), stderr: result.stderr() });
    } finally {
      await sandbox.delete();
    }
  } finally {
    client.close();
  }
}
```

A non-zero exit code is a normal result. Sandbox failures use `SandboxWsError`, whose `retryable` and `hint` fields can be mapped to a status code. Input validation runs before the lease is created.

Wire it into a route. This fence imports Hono, so it is not compiled with the rest:

```ts no-check
import { Hono } from 'hono';
import { SandboxWsError } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';
import { runShell, scriptSchema } from './run-shell.ts';

const app = new Hono();

app.post('/run', async (c) => {
  const parsed = z.object({ script: scriptSchema }).safeParse(await c.req.json().catch(() => undefined));
  if (!parsed.success) return c.json({ error: 'script must be a non-empty string' }, 400);
  try {
    return c.json(await runShell(parsed.data.script));
  } catch (err) {
    if (err instanceof SandboxWsError) {
      // 503 tells callers a retry may work, for example when no worker is online.
      return c.json({ error: err.code, hint: err.hint }, err.retryable ? 503 : 500);
    }
    throw err;
  }
});

export default app;
```

Authenticate the caller before this route runs anything. The sandbox contains the code; it does not decide who may ask for it.

## Mint a token for a browser

A page that talks to the sandbox gateway directly needs a token, not the API key. The gateway's `POST /v1/sessions` endpoint mints it. Do this on the server after you have authenticated the user.

```ts
import { z } from 'zod';

const grantSchema = z.object({ token: z.string().min(1), expires_at: z.number().int().positive() });
const browserSessionSchema = z.object({
  token: z.string().min(1),
  expiresAt: z.number().int().positive(),
  gatewayWsUrl: z
    .string()
    .url()
    .refine((url) => /^wss?:/.test(url)),
});
export type BrowserSession = z.infer<typeof browserSessionSchema>;

export async function mintBrowserSession(): Promise<BrowserSession> {
  const response = await fetch(`${process.env.SANDBOX_GATEWAY_HTTP_URL}/v1/sessions`, {
    method: 'POST',
    headers: { 'X-Sandbox-API-Key': process.env.SANDBOX_API_KEY! },
  });
  if (response.status !== 201) {
    throw new Error(`session mint failed with status ${response.status}`);
  }
  const body = grantSchema.parse(await response.json());
  return browserSessionSchema.parse({
    token: body.token,
    expiresAt: body.expires_at,
    gatewayWsUrl: process.env.SANDBOX_GATEWAY_WS_URL,
  });
}
```

```ts no-check
import { Hono } from 'hono';
import { mintBrowserSession } from './mint-session.ts';

const app = new Hono();

app.post('/api/sandbox-session', async (c) => {
  // Authenticate the user here before minting.
  return c.json(await mintBrowserSession());
});
```

The token carries the tenant's full access until `expiresAt` (unix seconds) and does not refresh. Mint per user or per page load, and ask again when the socket closes. The browser side is `SandboxWsClient.connectWithToken({ gatewayWsUrl, token })`; see the package README.

## Check capacity before leasing

`client.limits()` reports live leases against the cap and the monthly cpu and wall budgets, so a route can refuse early with a clear message instead of waiting for a denied lease.

```ts
import { SandboxWsClient } from '@trogonai/sandbox-ws-client';

export async function hasCapacity(client: SandboxWsClient): Promise<boolean> {
  const limits = await client.limits();
  return limits.active_leases < limits.max_active_leases && limits.cpu_seconds_used < limits.cpu_seconds_cap;
}
```

The check is advisory: another request can take the last slot between the check and the lease, so still handle a denied lease.

## Related

The runnable Hono example also validates streamed events, capacity replies and reaper options with Zod. It supports `local_docker` and a `memory` demo mode; another provider needs its own compatible image and capability checks. The memory demo runs no real shell or HTTP server.

- [Next.js guide](nextjs.md)
- [Package README](../README.md)
