# Use the sandbox client from Next.js

Next.js splits work between the server (Route Handlers, Server Actions) and the browser (`'use client'` components). The sandbox client fits both with one rule: the tenant API key stays on the server, and the browser only ever holds a short-lived session token.

Use Node 22.7 or newer and pnpm. The runnable Next.js example includes a production build that checks its server and browser entry points; run it for your dependency set before deployment.

For the runnable Docker example, preload `alpine:latest` on the worker's Docker daemon before creating leases. The framework examples use the same explicit `local_docker` or `memory` configuration as their package READMEs.

## Server: run code from a Server Action or Route Handler

Put the sandbox logic in a plain module so a Server Action and a Route Handler can share it. Read the connection settings from server-only environment variables (no `NEXT_PUBLIC_` prefix).

```ts
import { Sandbox, SandboxWsClient, tenantId } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

const commandSchema = z.object({ cmd: z.string().min(1), args: z.array(z.string()) });
const resultSchema = z.object({ exitCode: z.number().int(), stdout: z.string(), stderr: z.string() });
export type RunResult = z.infer<typeof resultSchema>;

export async function runInSandbox(cmd: string, args: string[]): Promise<RunResult> {
  const command = commandSchema.parse({ cmd, args });
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
      const result = await sandbox.runCommand({ ...command, timeoutSeconds: 30 });
      return resultSchema.parse({ exitCode: result.exitCode, stdout: result.stdout(), stderr: result.stderr() });
    } finally {
      await sandbox.delete();
    }
  } finally {
    client.close();
  }
}
```

Call it from a Server Action. This fence uses a Next.js directive and a framework import, so it is not compiled with the rest:

```ts no-check
'use server';

import { SandboxWsError } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';
import { runInSandbox } from '@/lib/run-in-sandbox.ts';

export async function runScript(formData: FormData) {
  const parsed = z
    .string()
    .refine((script) => script.trim().length > 0)
    .safeParse(formData.get('script'));
  if (!parsed.success) return { exitCode: -1, stdout: '', stderr: 'script must be text and must not be blank' };
  // Authenticate the user before running anything.
  try {
    return await runInSandbox('sh', ['-c', parsed.data]);
  } catch (err) {
    if (err instanceof SandboxWsError) {
      return { exitCode: -1, stdout: '', stderr: err.hint };
    }
    throw err;
  }
}
```

A sandbox run can take longer than your host's default function timeout. Set `export const maxDuration` on the route or page that calls it to at least the sandbox `timeoutMs` plus lease startup, or use a detached command and poll (`runCommand({ detached: true })`, then `command.wait()`).

## Server: mint a token for the browser

A Route Handler mints the session token. It calls the gateway's `POST /v1/sessions` with the API key and returns only the token.

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
    cache: 'no-store',
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
// app/api/sandbox-session/route.ts
import { NextResponse } from 'next/server';
import { mintBrowserSession } from '@/lib/mint-session.ts';

export const dynamic = 'force-dynamic';

export async function POST() {
  // Authenticate the user here before minting.
  return NextResponse.json(await mintBrowserSession());
}
```

The token carries the tenant's full access until `expiresAt` (unix seconds) and cannot be narrowed or refreshed. Mint per user or per page load.

## Browser: connect with the token

In a client component, fetch a token and connect. The API key is never in the bundle.

```ts
import { Sandbox, SandboxWsClient, tenantId } from '@trogonai/sandbox-ws-client';
import { z } from 'zod';

export async function runInBrowserSession(tenant: string, script: string): Promise<string> {
  const response = await fetch('/api/sandbox-session', { method: 'POST' });
  if (!response.ok) throw new Error('could not create a browser session');
  const { token, gatewayWsUrl } = z
    .object({ token: z.string().min(1), gatewayWsUrl: z.string().url() })
    .parse(await response.json());
  const client = await SandboxWsClient.connectWithToken({ gatewayWsUrl, token });
  try {
    const sandbox = await Sandbox.create(client, {
      tenant: tenantId(tenant),
      providerPref: ['local_docker'],
      timeoutMs: 60_000,
    });
    try {
      const result = await sandbox.runCommand('sh', ['-c', script]);
      return result.stdout();
    } finally {
      await sandbox.delete();
    }
  } finally {
    client.close();
  }
}
```

```ts no-check
'use client';

import { useState } from 'react';
import { runInBrowserSession } from '@/lib/run-in-browser-session.ts';

export function Runner() {
  const [out, setOut] = useState('');
  return (
    <button onClick={async () => setOut(await runInBrowserSession('acme', 'echo hello'))}>
      Run {out}
    </button>
  );
}
```

When the socket closes or the token expires, mint a new token and call `connectWithToken` again.

## Related

The runnable example validates route inputs, server-action form values, command results and gateway session grants with Zod. It supports `local_docker` and a `memory` demo mode. The latter has no real gateway or browser token and does not prove sandbox execution.

- [Hono guide](hono.md)
- [Package README](../README.md), including the bundling notes for Node, browser and edge targets
