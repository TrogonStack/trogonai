import { describe, expect, it } from 'vitest';

import {
  hasCapability,
  listSandboxes,
  MemorySandboxClient,
  reapSandboxes,
  Sandbox,
  SandboxCapabilities,
  tenantId,
} from '../src/index.ts';
import type { LeaseId, SandboxClientApi } from '../src/index.ts';

const tenant = tenantId('acme');

describe('listSandboxes and reapSandboxes', () => {
  it('filter by labels the same way list and cleanup filter by tags', async () => {
    const client = new MemorySandboxClient();
    const web = await Sandbox.create(client, { tenant, tags: { role: 'web' } });
    await Sandbox.create(client, { tenant, tags: { role: 'db' } });

    const found = await listSandboxes(client, { labels: { role: 'web' } });
    expect(found.map((s) => s.sandboxId)).toEqual([web.sandboxId]);
    expect(await listSandboxes(client)).toHaveLength(2);

    const report = await reapSandboxes(client, { labels: { role: 'web' }, dryRun: true });
    expect(report).toEqual(await Sandbox.cleanup(client, { tags: { role: 'web' }, dryRun: true }));
  });
});

describe('hasCapability', () => {
  it('accepts feature names and common aliases and narrows the handle', async () => {
    const client = new MemorySandboxClient();
    const sandbox = (await Sandbox.create(client, { tenant })).withCapabilities(
      SandboxCapabilities.from(['pause_resume', 'fork']),
    );
    expect(hasCapability(sandbox, 'pause')).toBe(true);
    expect(hasCapability(sandbox, 'pause_resume')).toBe(true);
    expect(hasCapability(sandbox, 'fork')).toBe(true);
    expect(hasCapability(sandbox, 'snapshot')).toBe(false);
    if (hasCapability(sandbox, 'pause')) {
      sandbox.capabilities.require('pause_resume');
    }
  });

  it('is false when capabilities are unknown', () => {
    const sandbox = new Sandbox(new MemorySandboxClient(), {
      sandboxId: 'lease-1' as LeaseId,
      provider: 'docker' as never,
      deadline: new Date(),
    });
    expect(sandbox.capabilities).toBeUndefined();
    expect(hasCapability(sandbox, 'pause')).toBe(false);
  });
});

describe('trackChanges and fs.mark', () => {
  it('reports changes since creation when trackChanges is set', async () => {
    const client = new MemorySandboxClient();
    const tracked = await Sandbox.create(client, { tenant, trackChanges: true });
    client.filesystem(tracked.sandboxId as LeaseId).write('/after.txt', 'y');
    const changes = await tracked.fs.changes();
    expect(changes.since).not.toBe('start');
    expect(changes.added).toEqual(['/after.txt']);

    const plain = await Sandbox.create(client, { tenant });
    expect((await plain.fs.changes()).since).toBe('start');
  });

  it('checkpoints the subtree trackChanges names', async () => {
    const memory = new MemorySandboxClient();
    const paths: string[] = [];
    const client = new Proxy(memory, {
      get(target, prop, receiver) {
        if (prop === 'checkpointFiles') {
          return async (_leaseId: LeaseId, path: string) => {
            paths.push(path);
            return { checkpointId: 'cp-x', entries: 0 };
          };
        }
        const value = Reflect.get(target, prop, receiver);
        return typeof value === 'function' ? value.bind(target) : value;
      },
    }) as SandboxClientApi;
    await Sandbox.create(client, { tenant, trackChanges: { path: '/workspace' } });
    expect(paths).toEqual(['/workspace']);
  });

  it('tracks a subtree that does not exist yet', async () => {
    const client = new MemorySandboxClient();
    const tracked = await Sandbox.create(client, { tenant, trackChanges: { path: '/workspace' } });
    const fs = client.filesystem(tracked.sandboxId as LeaseId);
    fs.write('/workspace/new.txt', 'x');
    fs.write('/elsewhere.txt', 'y');
    expect((await tracked.fs.changes({ path: '/workspace' })).added).toEqual(['/workspace/new.txt']);
  });

  it('diffs against a named mark and moves the mark when reused', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant });
    const fs = client.filesystem(sandbox.sandboxId as LeaseId);
    fs.write('/w/a.txt', 'one');
    expect(await sandbox.fs.mark('turn-1', { path: '/w' })).toBe('turn-1');
    fs.write('/w/b.txt', 'two');
    expect((await sandbox.fs.changes({ since: 'turn-1' })).added).toEqual(['/w/b.txt']);

    await sandbox.fs.mark('turn-1', { path: '/w' });
    fs.write('/w/c.txt', 'three');
    expect((await sandbox.fs.changes({ since: 'turn-1' })).added).toEqual(['/w/c.txt']);

    const unnamed = await sandbox.fs.mark(undefined, { path: '/w' });
    fs.remove('/w/a.txt');
    expect((await sandbox.fs.changes({ since: unnamed })).deleted).toEqual(['/w/a.txt']);
  });

  it('deletes the new sandbox when the baseline cannot be recorded', async () => {
    const memory = new MemorySandboxClient();
    const deleted: string[] = [];
    const client = new Proxy(memory, {
      get(target, prop, receiver) {
        if (prop === 'checkpointFiles') {
          return async () => {
            throw new Error('no change tracking here');
          };
        }
        if (prop === 'deleteLease') {
          return async (leaseId: LeaseId) => {
            deleted.push(leaseId);
            return target.deleteLease(leaseId);
          };
        }
        const value = Reflect.get(target, prop, receiver);
        return typeof value === 'function' ? value.bind(target) : value;
      },
    }) as SandboxClientApi;
    await expect(Sandbox.create(client, { tenant, trackChanges: true })).rejects.toThrow('no change tracking here');
    expect(deleted).toHaveLength(1);
    await expect(Sandbox.connect(client, deleted[0]!)).rejects.toMatchObject({ code: 'lease_not_found' });
  });
});
