import { describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';
import type { FileWatchItem, LeaseId } from '../src/index.ts';

async function create(client = new MemorySandboxClient()) {
  const sandbox = await Sandbox.create(client, { tenant: tenantId('acme'), timeoutMs: 60_000 });
  return { client, sandbox, fs: client.filesystem(sandbox.sandboxId as LeaseId) };
}

describe('memory fake change tracking', () => {
  it('reports exact added, modified and deleted paths since a checkpoint', async () => {
    const { sandbox, fs } = await create();
    fs.write('/w/keep.txt', 'one');
    fs.write('/w/edit.txt', 'one');
    fs.write('/w/gone.txt', 'one');
    const cp = await sandbox.fs.checkpoint('/w');
    expect(cp.entries).toBe(3);

    fs.write('/w/new.txt', 'x');
    fs.write('/w/edit.txt', 'two');
    fs.write('/w/keep.txt', 'one');
    fs.remove('/w/gone.txt');

    const changes = await sandbox.fs.changes({ since: cp.checkpointId });
    expect(changes).toEqual({
      since: cp.checkpointId,
      added: ['/w/new.txt'],
      modified: ['/w/edit.txt'],
      deleted: ['/w/gone.txt'],
      truncated: false,
    });
    expect((await sandbox.fs.changes({ since: cp.checkpointId, path: '/w/new.txt' })).added).toEqual(['/w/new.txt']);
  });

  it('measures from the lease start when no checkpoint is named', async () => {
    const { sandbox, fs } = await create();
    fs.write('/a.txt', 'x');
    const changes = await sandbox.fs.changes();
    expect(changes.since).toBe('start');
    expect(changes.added).toEqual(['/a.txt']);
  });

  it('does not share checkpoints between leases', async () => {
    const client = new MemorySandboxClient();
    const a = await create(client);
    const b = await create(client);
    a.fs.write('/w/f', 'x');
    const cp = await a.sandbox.fs.checkpoint('/w');
    await expect(b.sandbox.fs.changes({ since: cp.checkpointId })).rejects.toMatchObject({ code: 'invalid_request' });
  });

  it('streams watch events as files change and ends when aborted', async () => {
    const { sandbox, fs } = await create();
    fs.write('/w/a.txt', 'one');
    const abort = new AbortController();
    const seen: FileWatchItem[] = [];
    const reader = (async () => {
      for await (const item of sandbox.fs.watch('/w', { recursive: true, signal: abort.signal })) {
        seen.push(item);
        if (seen.length === 3) abort.abort();
      }
    })();
    await new Promise((r) => setTimeout(r, 0));
    fs.write('/w/sub/b.txt', 'x');
    await new Promise((r) => setTimeout(r, 0));
    fs.write('/w/a.txt', 'two');
    await new Promise((r) => setTimeout(r, 0));
    fs.remove('/w/sub/b.txt');
    await reader;
    expect(seen.map((i) => (i.type === 'event' ? [i.event.kind, i.event.path, i.event.seq] : i.type))).toEqual([
      ['created', '/w/sub/b.txt', 1],
      ['modified', '/w/a.txt', 2],
      ['deleted', '/w/sub/b.txt', 3],
    ]);
  });

  it('a non-recursive watch ignores deeper paths', async () => {
    const { sandbox, fs } = await create();
    fs.mkdirp('/w');
    const abort = new AbortController();
    const kinds: string[] = [];
    const reader = (async () => {
      for await (const item of sandbox.fs.watch('/w', { signal: abort.signal })) {
        if (item.type === 'event') {
          kinds.push(item.event.path);
          abort.abort();
        }
      }
    })();
    await new Promise((r) => setTimeout(r, 0));
    fs.write('/w/deep/x.txt', 'x');
    await new Promise((r) => setTimeout(r, 0));
    fs.write('/w/top.txt', 'x');
    await reader;
    expect(kinds).toEqual(['/w/top.txt']);
  });

  it('checkpoints a missing path with an empty baseline', async () => {
    const { sandbox } = await create();
    await expect(sandbox.fs.checkpoint('/missing')).resolves.toMatchObject({ entries: 0 });
  });

  it('refuses to watch a missing path', async () => {
    const { sandbox } = await create();
    const iterate = async () => {
      for await (const _ of sandbox.fs.watch('/missing')) {
        // never reached
      }
    };
    await expect(iterate()).rejects.toMatchObject({ code: 'not_found' });
  });
});
