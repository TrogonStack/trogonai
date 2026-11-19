import { describe, expect, it } from 'vitest';

import { tenantId } from '../src/ids.ts';
import { MemorySandboxClient } from '../src/memory.ts';
import { OWNER_TAG_KEY, OWNER_TAG_VALUE, ownershipFilter, withOwnershipTag } from '../src/ownership.ts';
import { Sandbox } from '../src/sandbox.ts';

const tenant = tenantId('acme');

describe('withOwnershipTag', () => {
  it('adds the tag to empty and existing tags', () => {
    expect(withOwnershipTag(undefined)).toEqual({ [OWNER_TAG_KEY]: OWNER_TAG_VALUE });
    expect(withOwnershipTag({ env: 'dev' })).toEqual({ env: 'dev', [OWNER_TAG_KEY]: OWNER_TAG_VALUE });
  });

  it('leaves tags alone when opted out', () => {
    expect(withOwnershipTag({ env: 'dev' }, false)).toEqual({ env: 'dev' });
    expect(withOwnershipTag(undefined, false)).toBeUndefined();
  });

  it('skips the tag when all five slots are taken', () => {
    const full = { a: '1', b: '2', c: '3', d: '4', e: '5' };
    expect(withOwnershipTag(full)).toEqual(full);
  });

  it('never overrides a caller-set client key', () => {
    expect(withOwnershipTag({ client: 'mine' })).toEqual({ client: 'mine' });
  });
});

describe('ownershipFilter', () => {
  it('merges the tag into a filter unless foreign leases are wanted', () => {
    expect(ownershipFilter({ env: 'dev' })).toEqual({ env: 'dev', [OWNER_TAG_KEY]: OWNER_TAG_VALUE });
    expect(ownershipFilter({ env: 'dev' }, true)).toEqual({ env: 'dev' });
    expect(ownershipFilter(undefined, true)).toBeUndefined();
  });
});

describe('ownership tag on Sandbox', () => {
  it('tags every created lease by default', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant, tags: { env: 'dev' } });
    expect(sandbox.tags).toEqual({ env: 'dev', client: 'sandbox-ws-client' });
    const { leases } = await client.listLeases();
    expect(leases[0]?.tags).toEqual({ env: 'dev', client: 'sandbox-ws-client' });
  });

  it('list defaults to owned leases and can include foreign ones', async () => {
    const client = new MemorySandboxClient();
    const mine = await Sandbox.create(client, { tenant });
    const optedOut = await Sandbox.create(client, { tenant, ownershipTag: false });
    const foreign = await Sandbox.create(client, { tenant, tags: { client: 'other-tool' } });
    expect((await Sandbox.list(client)).map((s) => s.sandboxId)).toEqual([mine.sandboxId]);
    const all = (await Sandbox.list(client, { includeForeign: true })).map((s) => s.sandboxId);
    expect(all).toEqual([mine.sandboxId, optedOut.sandboxId, foreign.sandboxId]);
  });

  it('list combines the ownership tag with caller tags', async () => {
    const client = new MemorySandboxClient();
    const dev = await Sandbox.create(client, { tenant, tags: { env: 'dev' } });
    await Sandbox.create(client, { tenant, tags: { env: 'prod' } });
    expect((await Sandbox.list(client, { tags: { env: 'dev' } })).map((s) => s.sandboxId)).toEqual([dev.sandboxId]);
  });

  it('cleanup releases only owned leases by default', async () => {
    const client = new MemorySandboxClient();
    const mine = await Sandbox.create(client, { tenant });
    const foreign = await Sandbox.create(client, { tenant, ownershipTag: false });
    const dry = await Sandbox.cleanup(client, { dryRun: true });
    expect(dry.outcome === 'reaped' && dry.matched.map((m) => m.lease_id)).toEqual([mine.sandboxId]);
    expect((await Sandbox.list(client, { includeForeign: true })).length).toBe(2);
    const done = await Sandbox.cleanup(client);
    expect(done.outcome === 'reaped' && done.released).toEqual([mine.sandboxId]);
    expect((await Sandbox.list(client, { includeForeign: true })).map((s) => s.sandboxId)).toEqual([foreign.sandboxId]);
  });

  it('cleanup with includeForeign needs another filter', async () => {
    const client = new MemorySandboxClient();
    await Sandbox.create(client, { tenant, ownershipTag: false });
    await expect(Sandbox.cleanup(client, { includeForeign: true })).rejects.toThrow(/at least one/);
    const done = await Sandbox.cleanup(client, { includeForeign: true, olderThanMs: 0 });
    expect(done.outcome === 'reaped' && done.released.length).toBe(1);
  });

  it('updateTags keeps the ownership tag when there is room', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant });
    await sandbox.updateTags({ env: 'dev' });
    const { leases } = await client.listLeases();
    expect(leases[0]?.tags).toEqual({ env: 'dev', client: 'sandbox-ws-client' });
  });
});
