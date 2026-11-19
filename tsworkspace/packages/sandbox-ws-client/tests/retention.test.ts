import { describe, expect, it } from 'vitest';

import type { SandboxClientApi } from '../src/client-api.ts';
import { classify } from '../src/error-classes.ts';
import { tenantId } from '../src/ids.ts';
import { Sandbox } from '../src/sandbox.ts';
import type { LeaseRequest } from '../src/wire.ts';

async function leaseSpec(opts: Parameters<typeof Sandbox.create>[1]): Promise<LeaseRequest['spec']> {
  let captured: LeaseRequest | undefined;
  const client = {
    lease: async (request: LeaseRequest) => {
      captured = request;
      return { lease_id: 'lease-1', provider: 'docker', deadline: { secs_since_epoch: 0, nanos_since_epoch: 0 } };
    },
  } as unknown as SandboxClientApi;
  await Sandbox.create(client, opts);
  if (captured === undefined) throw new Error('lease was not called');
  return captured.spec;
}

describe('retention toggles', () => {
  it('sends nothing while both toggles stay on', async () => {
    expect((await leaseSpec({ tenant: tenantId('t') })).retention).toBeUndefined();
    expect((await leaseSpec({ tenant: tenantId('t'), monitoring: true, snapshots: true })).retention).toBeUndefined();
  });

  it('sends both toggles once either is turned off', async () => {
    expect((await leaseSpec({ tenant: tenantId('t'), monitoring: false })).retention).toEqual({
      monitoring: false,
      snapshots: true,
    });
    expect((await leaseSpec({ tenant: tenantId('t'), snapshots: false })).retention).toEqual({
      monitoring: true,
      snapshots: false,
    });
  });

  it('classifies a snapshots_disabled denial as permanent', () => {
    expect(classify({ code: 'lease_denied', kind: 'snapshots_disabled' }).retryable).toBe(false);
  });
});
