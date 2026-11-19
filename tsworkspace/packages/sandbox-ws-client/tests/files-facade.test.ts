import { describe, expect, it, vi } from 'vitest';

import { EditRejectedError, MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';

async function fixture() {
  const client = new MemorySandboxClient();
  const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
  return { client, sandbox };
}

describe('structured file facade', () => {
  it('lists bounded recursive metadata without discarding truncation', async () => {
    const { client, sandbox } = await fixture();
    const listing = { entries: [{ path: '/work/a.ts', name: 'a.ts', kind: 'file', size: 3 }], truncated: true };
    const listDir = vi.fn(async () => listing);
    Object.assign(client, { listDir });
    expect(await sandbox.fs.list('/work', { recursive: true, maxEntries: 1, user: 'sandbox' })).toEqual(listing);
    expect(listDir).toHaveBeenCalledExactlyOnceWith(sandbox.sandboxId, '/work', {
      recursive: true,
      maxEntries: 1,
      asUser: 'sandbox',
    });
  });

  it('retains atomic edit rejection and caller identity', async () => {
    const { client, sandbox } = await fixture();
    const failure = new EditRejectedError(2, 1);
    const editFile = vi.fn(async () => {
      throw failure;
    });
    Object.assign(client, { editFile });
    await expect(
      sandbox.fs.edit('/work/a.ts', { find: 'before', replace: 'after', expectedOccurrences: 1, user: 'sandbox' }),
    ).rejects.toBe(failure);
    expect(editFile).toHaveBeenCalledExactlyOnceWith(sandbox.sandboxId, '/work/a.ts', 'before', 'after', {
      expectedOccurrences: 1,
      asUser: 'sandbox',
    });
  });

  it('searches literal text with explicit limits and preserves search metadata', async () => {
    const { client, sandbox } = await fixture();
    const result = { matches: [{ path: '/work/a.ts', line: 2, text: 'needle' }], truncated: true, filesScanned: 7 };
    const searchFiles = vi.fn(async () => result);
    Object.assign(client, { searchFiles });
    expect(await sandbox.fs.search('needle', { path: '/work', glob: '**/*.ts', maxResults: 1 })).toEqual(result);
    expect(searchFiles).toHaveBeenCalledExactlyOnceWith(sandbox.sandboxId, '/work', {
      pattern: 'needle',
      glob: '**/*.ts',
      maxResults: 1,
    });
  });

  it('rejects invalid file bounds before submitting an operation', async () => {
    const { client, sandbox } = await fixture();
    const listDir = vi.fn(async () => ({ entries: [], truncated: false }));
    Object.assign(client, { listDir });
    await expect(sandbox.fs.list('/work', { maxEntries: -1 })).rejects.toMatchObject({ code: 'invalid_request' });
    expect(listDir).not.toHaveBeenCalled();
  });

  it('rejects malformed entry metadata from process-independent file APIs', async () => {
    const { client, sandbox } = await fixture();
    Object.assign(client, {
      listDir: async () => ({ entries: [{ path: '/work/a', kind: 'invented', size: -1 }], truncated: false }),
    });
    await expect(sandbox.fs.list('/work')).rejects.toMatchObject({ code: 'invalid_response' });
  });

  it('edits, lists and searches the modeled filesystem through the same public facade', async () => {
    const { sandbox } = await fixture();
    await sandbox.writeFiles([
      { path: '/work/a.ts', content: 'first\nneedle\n' },
      { path: '/work/nested/b.ts', content: 'needle\n' },
      { path: '/work/c.txt', content: 'needle\n' },
    ]);
    await sandbox.fs.edit('/work/a.ts', { find: 'first', replace: 'changed' });
    expect(await sandbox.readFile({ path: '/work/a.ts' })).toBe('changed\nneedle\n');
    const listed = await sandbox.fs.list('/work', { recursive: true, maxEntries: 2 });
    expect(listed.entries).toHaveLength(2);
    expect(listed.truncated).toBe(true);
    const searched = await sandbox.fs.search('needle', { path: '/work', glob: '**/*.ts', maxResults: 1 });
    expect(searched.matches).toEqual([{ path: 'a.ts', line: 2, text: 'needle' }]);
    expect(searched.truncated).toBe(true);
    await expect(sandbox.fs.edit('/work/a.ts', { find: 'absent', replace: 'wrong' })).rejects.toBeInstanceOf(
      EditRejectedError,
    );
    expect(await sandbox.readFile({ path: '/work/a.ts' })).toBe('changed\nneedle\n');
  });
});
