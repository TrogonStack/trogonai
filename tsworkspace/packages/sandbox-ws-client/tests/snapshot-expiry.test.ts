import { describe, expect, it, vi } from 'vitest';

import { MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';

async function fixture() {
  const client = new MemorySandboxClient();
  const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
  const snapshot = vi.spyOn(client, 'snapshotLease');
  return { sandbox, snapshot };
}

describe('snapshot expiry authority', () => {
  it.each([-1, 0.5, Number.NaN, Number.POSITIVE_INFINITY])(
    'rejects invalid expiresInMs %s before taking a snapshot',
    async (expiresInMs) => {
      const { sandbox, snapshot } = await fixture();
      const outcome = await sandbox.snapshot({ expiresInMs }).then(
        () => 'created',
        (error: { code?: string }) => error.code,
      );
      expect(outcome).toBe('invalid_request');
      expect(snapshot).not.toHaveBeenCalled();
    },
  );

  it.each([0, 60_000])(
    'refuses an expiry promise the broker cannot represent (%s) before taking a snapshot',
    async (expiresInMs) => {
      const { sandbox, snapshot } = await fixture();
      const outcome = await sandbox.snapshot({ expiresInMs }).then(
        () => 'created',
        (error: { code?: string }) => error.code,
      );
      expect(outcome).toBe('unsupported');
      expect(snapshot).not.toHaveBeenCalled();
    },
  );
});
