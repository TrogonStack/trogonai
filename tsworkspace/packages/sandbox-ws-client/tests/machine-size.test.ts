import { describe, expect, it } from 'vitest';

import type { SandboxClientApi } from '../src/client-api.ts';
import { tenantId } from '../src/ids.ts';
import { MACHINE_SIZE_PRESETS, Sandbox } from '../src/sandbox.ts';
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

describe('machine size', () => {
  it('sends no size fields by default', async () => {
    const spec = await leaseSpec({ tenant: tenantId('t') });
    expect(spec.vcpu_millis).toBeUndefined();
    expect(spec.disk_mib).toBeUndefined();
  });

  it.each(Object.entries(MACHINE_SIZE_PRESETS))('maps the %s preset to numbers', async (name, numbers) => {
    const spec = await leaseSpec({ tenant: tenantId('t'), size: name as keyof typeof MACHINE_SIZE_PRESETS });
    expect(spec.vcpu_millis).toBe(numbers.vcpu_millis);
    expect(spec.memory_mib).toBe(numbers.memory_mib);
    expect(spec.disk_mib).toBeUndefined();
  });

  it('lets explicit values override a preset', async () => {
    const spec = await leaseSpec({ tenant: tenantId('t'), size: 'large', vcpus: 0.5, memoryMib: 256, diskMib: 2048 });
    expect(spec.vcpu_millis).toBe(500);
    expect(spec.memory_mib).toBe(256);
    expect(spec.disk_mib).toBe(2048);
  });

  it('rejects a non-positive vcpu count', async () => {
    await expect(leaseSpec({ tenant: tenantId('t'), vcpus: 0 })).rejects.toThrow(RangeError);
  });
});
