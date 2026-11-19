import { describe, expect, it } from 'vitest';

import type { SandboxClientApi } from '../src/client-api.ts';
import { classify } from '../src/error-classes.ts';
import { tenantId } from '../src/ids.ts';
import { Sandbox } from '../src/sandbox.ts';
import type { LeaseRequest } from '../src/wire.ts';

async function leaseRequest(opts: Parameters<typeof Sandbox.create>[1]): Promise<LeaseRequest> {
  let captured: LeaseRequest | undefined;
  const client = {
    lease: async (request: LeaseRequest) => {
      captured = request;
      return { lease_id: 'lease-1', provider: 'docker', deadline: { secs_since_epoch: 0, nanos_since_epoch: 0 } };
    },
  } as unknown as SandboxClientApi;
  await Sandbox.create(client, opts);
  if (captured === undefined) throw new Error('lease was not called');
  return captured;
}

describe('no-TTL leases', () => {
  it('sends nothing unless asked', async () => {
    expect((await leaseRequest({ tenant: tenantId('t') })).spec.no_ttl).toBeUndefined();
  });

  it('sends no_ttl alongside the idle timeout', async () => {
    const request = await leaseRequest({ tenant: tenantId('t'), noTtl: true, idleTimeoutMs: 60_000 });
    expect(request.spec.no_ttl).toBe(true);
    expect(request.spec.idle_timeout_seconds).toBe(60);
  });

  it('refuses noTtl without an idle timeout before any round trip', async () => {
    await expect(leaseRequest({ tenant: tenantId('t'), noTtl: true })).rejects.toThrow(RangeError);
  });

  it('classifies a lifetime cap denial as permanent', () => {
    expect(classify({ code: 'lease_denied', kind: 'would_exceed_max_lifetime' }).retryable).toBe(false);
  });
});

describe('idle policy', () => {
  it('sends the stages in whole seconds', async () => {
    const request = await leaseRequest({
      tenant: tenantId('t'),
      idlePolicy: [
        { afterMs: 299_500, action: 'pause' },
        { afterMs: 3_600_000, action: 'release' },
      ],
    });
    expect(request.spec.idle_policy).toEqual({
      stages: [
        { after_seconds: 300, action: 'pause' },
        { after_seconds: 3600, action: 'release' },
      ],
    });
    expect(request.spec.idle_timeout_seconds).toBeUndefined();
  });

  it('refuses an idle policy together with an idle timeout', async () => {
    await expect(
      leaseRequest({
        tenant: tenantId('t'),
        idleTimeoutMs: 60_000,
        idlePolicy: [{ afterMs: 60_000, action: 'pause' }],
      }),
    ).rejects.toThrow(RangeError);
  });

  it('lets a release stage stand in for the idle timeout of a no-TTL lease', async () => {
    const request = await leaseRequest({
      tenant: tenantId('t'),
      noTtl: true,
      idlePolicy: [{ afterMs: 600_000, action: 'release' }],
    });
    expect(request.spec.no_ttl).toBe(true);
    await expect(
      leaseRequest({ tenant: tenantId('t'), noTtl: true, idlePolicy: [{ afterMs: 600_000, action: 'pause' }] }),
    ).rejects.toThrow(RangeError);
  });
});
