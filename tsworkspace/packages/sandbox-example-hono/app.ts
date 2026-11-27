import { Sandbox, SandboxWsError } from '@trogonai/sandbox-ws-client';
import { Hono } from 'hono';
import { stream } from 'hono/streaming';

import {
  capacitySchema,
  errorResponseSchema,
  reapQuerySchema,
  reapResultSchema,
  scriptInputSchema,
  streamEventSchema,
} from './server/contracts.ts';
import { runShell, streamShell } from './server/run.ts';
import type { StreamEvent } from './server/run.ts';
import { isMemoryMode, sharedClient } from './server/sandbox.ts';
import { mintBrowserSession } from './server/session.ts';

export const app = new Hono();

// 503 tells callers a retry may work, for example when no worker is online.
app.onError((err, c) => {
  if (err instanceof SandboxWsError) {
    return c.json(errorResponseSchema.parse({ error: err.code, hint: err.hint }), err.retryable ? 503 : 500);
  }
  console.error(err);
  return c.json(errorResponseSchema.parse({ error: 'internal error' }), 500);
});

app.get('/health', (c) => c.json({ ok: true, mode: isMemoryMode() ? 'memory' : 'gateway' }));

/** Parses `{ script }` from the body; a real app authenticates the caller before this runs. */
async function readScript(request: Request): Promise<string | undefined> {
  const parsed = scriptInputSchema.safeParse(await request.json().catch(() => undefined));
  return parsed.success ? parsed.data.script : undefined;
}

app.post('/run', async (c) => {
  const script = await readScript(c.req.raw);
  if (script === undefined) {
    return c.json(errorResponseSchema.parse({ error: 'script must be a non-empty string' }), 400);
  }
  return c.json(await runShell(script));
});

// Newline-delimited JSON: one `StreamEvent` per line, so a client can render output while the command runs.
app.post('/run/stream', async (c) => {
  const script = await readScript(c.req.raw);
  if (script === undefined) {
    return c.json(errorResponseSchema.parse({ error: 'script must be a non-empty string' }), 400);
  }
  c.header('Content-Type', 'application/x-ndjson');
  return stream(c, async (out) => {
    const write = (event: StreamEvent) => out.write(`${JSON.stringify(streamEventSchema.parse(event))}\n`);
    // The status line is already sent, so a failure has to travel in the body.
    try {
      for await (const event of streamShell(script)) {
        await write(event);
      }
    } catch (err) {
      await write({ type: 'error', message: err instanceof SandboxWsError ? err.hint : 'internal error' });
    }
  });
});

app.post('/api/sandbox-session', async (c) => {
  // Authenticate the user here before minting.
  if (isMemoryMode()) {
    return c.json(errorResponseSchema.parse({ error: 'memory mode has no gateway to mint a token from' }), 501);
  }
  return c.json(await mintBrowserSession());
});

// Advisory: another request can take the last slot between this check and the lease.
app.get('/capacity', async (c) => {
  const limits = await (await sharedClient()).limits();
  const available = limits.active_leases < limits.max_active_leases && limits.cpu_seconds_used < limits.cpu_seconds_cap;
  return c.json(
    capacitySchema.parse({ available, activeLeases: limits.active_leases, maxActiveLeases: limits.max_active_leases }),
  );
});

// Releases this SDK's leases older than the cutoff. `?dryRun=1` only reports.
app.post('/admin/reap', async (c) => {
  const token = process.env.ADMIN_TOKEN;
  if (token === undefined || token === '' || c.req.header('authorization') !== `Bearer ${token}`) {
    return c.json(errorResponseSchema.parse({ error: 'unauthorized' }), 401);
  }
  const parsed = reapQuerySchema.safeParse({
    maxAgeSeconds: c.req.query('maxAgeSeconds'),
    dryRun: c.req.query('dryRun'),
  });
  if (!parsed.success) {
    return c.json(errorResponseSchema.parse({ error: 'invalid reap options' }), 400);
  }
  const reaped = await Sandbox.cleanup(await sharedClient(), {
    olderThanMs: parsed.data.maxAgeSeconds * 1000,
    dryRun: parsed.data.dryRun,
  });
  if (reaped.outcome === 'denied') {
    return c.json(errorResponseSchema.parse({ error: 'the broker denied the reap' }), 403);
  }
  return c.json(
    reapResultSchema.parse({
      dryRun: reaped.dry_run,
      matched: reaped.matched.length,
      released: reaped.released,
      failed: reaped.failed,
    }),
  );
});
