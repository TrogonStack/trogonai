import { Sandbox } from '@trogonai/sandbox-ws-client';

import { errorResponseSchema, reapQuerySchema, reapResultSchema } from '../../../server/contracts.ts';
import { sharedClient } from '../../../server/sandbox.ts';

export const dynamic = 'force-dynamic';

/**
 * Cron target: releases this SDK's leases older than the cutoff, so a run that died before its
 * `finally` does not hold a lease until the broker's own timeout. `vercel.json` schedules it.
 * `?dryRun=1` only reports; `?maxAgeSeconds=N` overrides the cutoff.
 */
export async function GET(request: Request): Promise<Response> {
  const secret = process.env.CRON_SECRET;
  if (secret === undefined || secret === '' || request.headers.get('authorization') !== `Bearer ${secret}`) {
    return Response.json(errorResponseSchema.parse({ error: 'unauthorized' }), { status: 401 });
  }
  const params = new URL(request.url).searchParams;
  const parsed = reapQuerySchema.safeParse({
    maxAgeSeconds: params.get('maxAgeSeconds') ?? undefined,
    dryRun: params.get('dryRun') ?? undefined,
  });
  if (!parsed.success) {
    return Response.json(errorResponseSchema.parse({ error: 'invalid reap options' }), { status: 400 });
  }
  const reaped = await Sandbox.cleanup(await sharedClient(), {
    olderThanMs: parsed.data.maxAgeSeconds * 1000,
    dryRun: parsed.data.dryRun,
  });
  if (reaped.outcome === 'denied') {
    return Response.json(errorResponseSchema.parse({ error: 'the broker denied the reap' }), { status: 403 });
  }
  return Response.json(
    reapResultSchema.parse({
      dryRun: reaped.dry_run,
      matched: reaped.matched.length,
      released: reaped.released,
      failed: reaped.failed,
    }),
  );
}
