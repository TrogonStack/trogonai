import { describe, expect, it } from 'vitest';

import { SandboxNotFoundError } from '../src/errors.ts';
import { tenantId } from '../src/ids.ts';
import { MemorySandboxClient } from '../src/memory.ts';
import { Sandbox } from '../src/sandbox.ts';

describe('Sandbox.connect', () => {
  it('attaches to an existing lease by id', async () => {
    const client = new MemorySandboxClient();
    const created = await Sandbox.create(client, { tenant: tenantId('acme'), name: 'job' });
    const attached = await Sandbox.connect(client, created.sandboxId);
    expect(attached.sandboxId).toBe(created.sandboxId);
    expect(attached.provider).toBe(created.provider);
    const result = await attached.runCommand('echo', ['hi']);
    expect(result.stdout()).toBe('hi\n');
  });

  it('throws a typed, non-retryable not-found for an unknown id', async () => {
    const client = new MemorySandboxClient();
    const err = await Sandbox.connect(client, 'nope').catch((e: unknown) => e);
    expect(err).toBeInstanceOf(SandboxNotFoundError);
    expect((err as SandboxNotFoundError).code).toBe('lease_not_found');
    expect((err as SandboxNotFoundError).retryable).toBe(false);
    expect((err as SandboxNotFoundError).leaseId).toBe('nope');
  });

  it('does not find a released lease', async () => {
    const client = new MemorySandboxClient();
    const created = await Sandbox.create(client, { tenant: tenantId('acme') });
    await created.delete();
    await expect(Sandbox.connect(client, created.sandboxId)).rejects.toBeInstanceOf(SandboxNotFoundError);
  });
});
