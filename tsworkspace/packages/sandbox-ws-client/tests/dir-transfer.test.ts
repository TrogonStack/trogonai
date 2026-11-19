import { createHash } from 'node:crypto';

import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { FILE_TRANSFER_CHUNK_BYTES, parseFileOpResponse } from '../src/fileops.ts';
import { leaseId } from '../src/ids.ts';
import { MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';
import type { FileOp } from '../src/wire.ts';
import {
  connectTestClient,
  onRequest,
  respond,
  startFakeServer,
  stopFakeServer,
  type ServerContext,
} from './harness.ts';

const LEASE = leaseId('lease-transfer');

function pattern(len: number): Uint8Array {
  return Uint8Array.from({ length: len }, (_, i) => (i * 31 + Math.floor(i / 7)) % 251);
}

function sha(bytes: Uint8Array): string {
  return createHash('sha256').update(bytes).digest('hex');
}

/** A file-op server backed by a map, recording every op it served. */
function serve(ctx: ServerContext, files: Map<string, Uint8Array>, corruptReads = false) {
  const ops: FileOp[] = [];
  onRequest(ctx, (frame, socket) => {
    const op = (frame.payload as { op: FileOp }).op;
    ops.push(op);
    switch (op.op) {
      case 'stat': {
        const data = files.get(op.path);
        if (data === undefined) return respond(socket, frame, { outcome: 'not_found', path: op.path });
        return respond(socket, frame, {
          outcome: 'stat',
          stat: { path: op.path, name: op.path, kind: 'file', size: data.length, mode: 0o644 },
        });
      }
      case 'checksum': {
        const data = files.get(op.path);
        if (data === undefined) return respond(socket, frame, { outcome: 'not_found', path: op.path });
        const start = Math.min(op.offset ?? 0, data.length);
        const end = op.length === undefined ? data.length : Math.min(start + op.length, data.length);
        return respond(socket, frame, {
          outcome: 'checksum',
          sha256: sha(data.subarray(start, end)),
          bytes: end - start,
          total_size: data.length,
        });
      }
      case 'write': {
        const incoming = Buffer.from(op.data, 'base64');
        const old = op.truncate === true ? new Uint8Array(0) : (files.get(op.path) ?? new Uint8Array(0));
        const offset = op.offset ?? 0;
        const next = new Uint8Array(Math.max(old.length, offset + incoming.length));
        next.set(old);
        next.set(incoming, offset);
        files.set(op.path, next);
        return respond(socket, frame, { outcome: 'written', bytes: incoming.length, size: next.length });
      }
      case 'read': {
        const data = files.get(op.path);
        if (data === undefined) return respond(socket, frame, { outcome: 'not_found', path: op.path });
        const offset = op.offset ?? 0;
        const end = Math.min(offset + 1000, data.length);
        const slice = Uint8Array.from(data.subarray(offset, end));
        if (corruptReads && slice.length > 0) slice[0] ^= 0xff;
        return respond(socket, frame, {
          outcome: 'content',
          data: Buffer.from(slice).toString('base64'),
          offset,
          total_size: data.length,
          eof: end >= data.length,
          truncated: false,
        });
      }
      case 'mkdir':
        return respond(socket, frame, { outcome: 'created', already_existed: false });
      case 'list': {
        const prefix = `${op.path}/`;
        const entries = [...files]
          .filter(([path]) => path.startsWith(prefix))
          .map(([path, data]) => ({
            path: path.slice(prefix.length),
            name: path.split('/').pop(),
            kind: 'file',
            size: data.length,
            mode: 0o644,
          }));
        return respond(socket, frame, { outcome: 'listing', entries, truncated: false });
      }
      default:
        return respond(socket, frame, { outcome: 'unsupported', message: 'no' });
    }
  });
  return ops;
}

function writes(ops: FileOp[]): Array<[number, boolean]> {
  return ops.flatMap((op) => (op.op === 'write' ? [[op.offset ?? 0, op.truncate === true] as [number, boolean]] : []));
}

describe('verified, resumable transfer', () => {
  let ctx: ServerContext;

  beforeEach(async () => {
    ctx = await startFakeServer();
  });

  afterEach(async () => {
    await stopFakeServer(ctx);
  });

  it('parses the checksum reply and rejects a malformed one', () => {
    expect(parseFileOpResponse({ outcome: 'checksum', sha256: 'ab', bytes: 1, total_size: 2 })).toMatchObject({
      outcome: 'checksum',
    });
    expect(() => parseFileOpResponse({ outcome: 'checksum', sha256: 'ab' })).toThrow();
  });

  it('uploads in chunks and verifies the digest', async () => {
    const files = new Map<string, Uint8Array>();
    const ops = serve(ctx, files);
    const client = await connectTestClient(ctx);
    const data = pattern(FILE_TRANSFER_CHUNK_BYTES * 2 + 123);
    const report = await client.uploadFileVerified(LEASE, '/big.bin', data);
    expect(report).toMatchObject({ files: 1, bytesTransferred: data.length, resumedFiles: 0 });
    expect(sha(files.get('/big.bin') as Uint8Array)).toBe(sha(data));
    expect(writes(ops)).toEqual([
      [0, true],
      [FILE_TRANSFER_CHUNK_BYTES, false],
      [FILE_TRANSFER_CHUNK_BYTES * 2, false],
    ]);
    client.close();
  });

  it('resumes after a matching prefix and restarts when it does not match', async () => {
    const files = new Map<string, Uint8Array>();
    const ops = serve(ctx, files);
    const client = await connectTestClient(ctx);
    const data = pattern(FILE_TRANSFER_CHUNK_BYTES + 10);
    files.set('/f.bin', data.slice(0, FILE_TRANSFER_CHUNK_BYTES));
    const report = await client.uploadFileVerified(LEASE, '/f.bin', data);
    expect(report.resumedFiles).toBe(1);
    expect(report.bytesTransferred).toBe(10);
    expect(writes(ops)).toEqual([[FILE_TRANSFER_CHUNK_BYTES, false]]);

    ops.length = 0;
    files.set('/g.bin', new Uint8Array(400).fill(9));
    const restarted = await client.uploadFileVerified(LEASE, '/g.bin', pattern(1000));
    expect(restarted.resumedFiles).toBe(0);
    expect(writes(ops)).toEqual([[0, true]]);
    expect(sha(files.get('/g.bin') as Uint8Array)).toBe(sha(pattern(1000)));

    ops.length = 0;
    const again = await client.uploadFileVerified(LEASE, '/g.bin', pattern(1000));
    expect(again.skippedFiles).toBe(1);
    expect(writes(ops)).toEqual([]);
    client.close();
  });

  it('downloads with a partial prefix and fails on a corrupt copy', async () => {
    const files = new Map<string, Uint8Array>();
    const data = pattern(2500);
    files.set('/f.bin', data);
    serve(ctx, files);
    const client = await connectTestClient(ctx);
    const resumed = await client.downloadFileVerified(LEASE, '/f.bin', { partial: data.slice(0, 1000) });
    expect(sha(resumed.data)).toBe(sha(data));
    expect(resumed.report).toMatchObject({ resumedFiles: 1, bytesTransferred: 1500 });
    const restarted = await client.downloadFileVerified(LEASE, '/f.bin', { partial: new Uint8Array(500).fill(1) });
    expect(restarted.report).toMatchObject({ resumedFiles: 0, bytesTransferred: 2500 });
    client.close();

    const bad = await startFakeServer();
    try {
      serve(bad, files, true);
      const badClient = await connectTestClient(bad);
      await expect(badClient.downloadFileVerified(LEASE, '/f.bin')).rejects.toThrow(/integrity check failed/);
      badClient.close();
    } finally {
      await stopFakeServer(bad);
    }
  });

  it('streams an upload in re-cut chunks and verifies the digest', async () => {
    const files = new Map<string, Uint8Array>();
    const ops = serve(ctx, files);
    const client = await connectTestClient(ctx);
    const data = pattern(FILE_TRANSFER_CHUNK_BYTES * 2 + 77);
    async function* pieces() {
      for (let at = 0; at < data.length; at += 100_003) yield data.subarray(at, Math.min(at + 100_003, data.length));
    }
    const report = await client.uploadFileStream(LEASE, '/s.bin', pieces());
    expect(report).toMatchObject({ files: 1, bytesTransferred: data.length });
    expect(sha(files.get('/s.bin') as Uint8Array)).toBe(sha(data));
    expect(writes(ops)).toEqual([
      [0, true],
      [FILE_TRANSFER_CHUNK_BYTES, false],
      [FILE_TRANSFER_CHUNK_BYTES * 2, false],
    ]);

    ops.length = 0;
    await client.uploadFileStream(LEASE, '/empty.bin', []);
    expect(writes(ops)).toEqual([[0, true]]);
    expect(files.get('/empty.bin')?.length).toBe(0);
    client.close();
  });

  it('resumes a stream upload from the matching prefix and refuses a mismatched one', async () => {
    const files = new Map<string, Uint8Array>();
    const ops = serve(ctx, files);
    const client = await connectTestClient(ctx);
    const data = pattern(FILE_TRANSFER_CHUNK_BYTES * 2 + 77);
    const cut = FILE_TRANSFER_CHUNK_BYTES + 5;
    files.set('/r.bin', data.slice(0, cut));
    async function* pieces() {
      for (let at = 0; at < data.length; at += 100_003) yield data.subarray(at, Math.min(at + 100_003, data.length));
    }
    ops.length = 0;
    const report = await client.uploadFileStream(LEASE, '/r.bin', pieces(), { resume: true });
    expect(report).toMatchObject({ resumedFiles: 1, bytesTransferred: data.length - cut });
    expect(sha(files.get('/r.bin') as Uint8Array)).toBe(sha(data));
    expect(writes(ops)[0]).toEqual([cut, false]);

    ops.length = 0;
    const again = await client.uploadFileStream(LEASE, '/r.bin', pieces(), { resume: true });
    expect(again).toMatchObject({ skippedFiles: 1, bytesTransferred: 0 });
    expect(writes(ops)).toEqual([]);

    files.set('/bad.bin', new Uint8Array(300).fill(7));
    ops.length = 0;
    await expect(client.uploadFileStream(LEASE, '/bad.bin', pieces(), { resume: true })).rejects.toThrow(
      /cannot resume/,
    );
    expect(writes(ops)).toEqual([]);

    files.set('/long.bin', new Uint8Array(data.length + 10));
    await expect(client.uploadFileStream(LEASE, '/long.bin', pieces(), { resume: true })).rejects.toThrow(
      /cannot resume/,
    );
    client.close();
  });

  it('streams a download page by page and fails on a corrupt copy', async () => {
    const files = new Map<string, Uint8Array>();
    const data = pattern(2500);
    files.set('/f.bin', data);
    serve(ctx, files);
    const client = await connectTestClient(ctx);
    const stream = client.downloadFileStream(LEASE, '/f.bin');
    const pages: Uint8Array[] = [];
    let step = await stream.next();
    while (!step.done) {
      pages.push(step.value);
      step = await stream.next();
    }
    expect(pages.length).toBe(3);
    expect(sha(Buffer.concat(pages))).toBe(sha(data));
    expect(step.value).toMatchObject({ files: 1, bytesTransferred: 2500 });
    client.close();

    const bad = await startFakeServer();
    try {
      serve(bad, files, true);
      const badClient = await connectTestClient(bad);
      const consume = async () => {
        for await (const _ of badClient.downloadFileStream(LEASE, '/f.bin')) {
          // drain
        }
      };
      await expect(consume()).rejects.toThrow(/integrity check failed/);
      const checked = badClient.downloadFileStream(LEASE, '/f.bin', { verifyPages: true });
      await expect(checked.next()).rejects.toThrow(/integrity check failed/);
      badClient.close();
    } finally {
      await stopFakeServer(bad);
    }
  });

  it('checks each page against a ranged digest before yielding it when asked', async () => {
    const files = new Map<string, Uint8Array>();
    const data = pattern(2500);
    files.set('/f.bin', data);
    const ops = serve(ctx, files);
    const client = await connectTestClient(ctx);
    const pages: Uint8Array[] = [];
    for await (const page of client.downloadFileStream(LEASE, '/f.bin', { verifyPages: true })) pages.push(page);
    expect(sha(Buffer.concat(pages))).toBe(sha(data));
    const ranged = ops.filter((op) => op.op === 'checksum' && op.length !== undefined);
    expect(ranged.map((op) => op.op === 'checksum' && [op.offset, op.length])).toEqual([
      [0, 1000],
      [1000, 1000],
      [2000, 500],
    ]);
    client.close();
  });

  it('round-trips a directory tree and refuses paths that escape it', async () => {
    const files = new Map<string, Uint8Array>();
    serve(ctx, files);
    const client = await connectTestClient(ctx);
    const report = await client.uploadDir(
      LEASE,
      '/tree',
      [
        { path: 'top.txt', data: new TextEncoder().encode('top') },
        { path: 'a/b/deep.bin', data: pattern(3000), mode: 0o755 },
      ],
      { emptyDirs: ['empty'] },
    );
    expect(report.files).toBe(2);
    expect(report.directories).toBe(2);
    const down = await client.downloadDir(LEASE, '/tree');
    expect(down.files.map((f) => f.path)).toEqual(['a/b/deep.bin', 'top.txt']);
    expect(sha(down.files[0].data)).toBe(sha(pattern(3000)));

    await expect(
      client.uploadDir(LEASE, '/tree', [{ path: '../evil', data: new Uint8Array(1) }]),
    ).rejects.toMatchObject({
      kind: 'invalid_path',
    });
    await expect(client.uploadDir(LEASE, '/tree', [{ path: '/abs', data: new Uint8Array(1) }])).rejects.toMatchObject({
      kind: 'invalid_path',
    });
    client.close();
  });
});

describe('Sandbox.fs transfer on the in-memory client', () => {
  it('uploads and downloads a directory through sandbox.fs', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme'), timeoutMs: 60_000 });
    const report = await sandbox.fs.uploadDir('/w', [
      { path: 'a.txt', data: new TextEncoder().encode('one') },
      { path: 'sub/b.txt', data: new TextEncoder().encode('two') },
    ]);
    expect(report.files).toBe(2);
    const down = await sandbox.fs.downloadDir('/w');
    expect(down.files.map((f) => f.path)).toEqual(['a.txt', 'sub/b.txt']);
    expect(down.directories).toEqual(['sub']);

    await sandbox.fs.uploadFile('/w/c.bin', new Uint8Array([1, 2, 3]));
    const again = await sandbox.fs.uploadFile('/w/c.bin', new Uint8Array([1, 2, 3]));
    expect(again.skippedFiles).toBe(1);
    const file = await sandbox.fs.downloadFile('/w/c.bin');
    expect([...file.data]).toEqual([1, 2, 3]);

    await sandbox.fs.uploadFileStream('/w/d.bin', [new Uint8Array([4, 5]), new Uint8Array([6])]);
    const pages: number[] = [];
    for await (const page of sandbox.fs.downloadFileStream('/w/d.bin')) pages.push(...page);
    expect(pages).toEqual([4, 5, 6]);
  });
});
