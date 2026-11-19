import { describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';
import type { CapabilityRequirementsInput, SandboxFeature } from '../src/index.ts';

const tenant = tenantId('acme');

describe('memory fake capability reporting', () => {
  it.each<[CapabilityRequirementsInput, SandboxFeature]>([
    [{ needs_snapshot: true }, 'snapshots'],
    [{ needs_change_tracking: true }, 'change_tracking'],
    [{ needs_file_watch: true }, 'file_watch'],
    [{ needs_dir_transfer: true }, 'dir_transfer'],
    [{ needs_graceful_stop: true }, 'graceful_stop'],
  ])('reports the modeled capability it admitted: %j', async (requiredCapabilities, feature) => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant, requiredCapabilities });
    expect(sandbox.supports(feature)).toBe(true);
  });

  it('admits and reports the literal search behavior it now models', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant, requiredCapabilities: { needs_file_search: true } });
    expect(sandbox.supports('file_search')).toBe(true);
    await sandbox.writeFiles([{ path: '/work/a.txt', content: 'needle' }]);
    expect((await sandbox.fs.search('needle', { path: '/work' })).matches).toEqual([
      { path: 'a.txt', line: 1, text: 'needle' },
    ]);
  });

  it('does not claim the general file-operations suite or browser/readiness support from partial implementations', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant });
    expect(sandbox.supports('file_ops')).toBe(false);
    expect(sandbox.supports('browser')).toBe(false);
    await expect(
      Sandbox.create(client, { tenant, requiredCapabilities: { needs_file_ops: true } }),
    ).rejects.toMatchObject({ code: 'lease_denied' });
    await expect(
      Sandbox.create(client, { tenant, requiredCapabilities: { needs_process_readiness: true } }),
    ).rejects.toMatchObject({ code: 'lease_denied' });
  });
});
