import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import {
  EditRejectedError,
  FILE_TRANSFER_CHUNK_BYTES,
  FileNotFoundError,
  FileOpFailedError,
  FileOpUnsupportedError,
  bytesToBase64,
  parseFileOpResponse,
} from '../src/fileops.ts';
import { leaseId } from '../src/ids.ts';
import type { FileOp } from '../src/wire.ts';
import {
  connectTestClient,
  onRequest,
  respond,
  startFakeServer,
  stopFakeServer,
  type ServerContext,
} from './harness.ts';

const LEASE = leaseId('lease-files');

describe('file_op wrappers', () => {
  let ctx: ServerContext;

  beforeEach(async () => {
    ctx = await startFakeServer();
  });

  afterEach(async () => {
    await stopFakeServer(ctx);
  });

  it('sends one tagged op and decodes a ranged read', async () => {
    let seen: { tool: string; payload: unknown } | undefined;
    onRequest(ctx, (frame, socket) => {
      seen = { tool: frame.tool, payload: frame.payload };
      respond(socket, frame, {
        outcome: 'content',
        data: bytesToBase64(new TextEncoder().encode('hello')),
        offset: 2,
        total_size: 9,
        eof: false,
        truncated: false,
      });
    });
    const client = await connectTestClient(ctx);
    const range = await client.readRange(LEASE, 'a.txt', { offset: 2, length: 5 });
    expect(new TextDecoder().decode(range.data)).toBe('hello');
    expect(range.totalSize).toBe(9);
    expect(range.eof).toBe(false);
    expect(seen?.tool).toBe('sandbox_file_op');
    const payload = seen?.payload as { lease_id: string; op: FileOp };
    expect(payload.op).toMatchObject({ op: 'read', path: 'a.txt', offset: 2, length: 5 });
    client.close();
  });

  it('turns an edit_rejected reply into EditRejectedError', async () => {
    onRequest(ctx, (frame, socket) => {
      respond(socket, frame, { outcome: 'edit_rejected', occurrences: 3, expected: 1 });
    });
    const client = await connectTestClient(ctx);
    const err = await client.editFile(LEASE, 'a.txt', 'x', 'y').catch((e: unknown) => e);
    expect(err).toBeInstanceOf(EditRejectedError);
    expect((err as EditRejectedError).occurrences).toBe(3);
    client.close();
  });

  it('maps typed refusals to errors', async () => {
    const replies = [
      { outcome: 'unsupported', message: 'no files here' },
      { outcome: 'not_found', path: 'gone' },
      { outcome: 'failed', kind: 'too_large', message: 'big' },
    ];
    let n = 0;
    onRequest(ctx, (frame, socket) => {
      respond(socket, frame, replies[n++]);
    });
    const client = await connectTestClient(ctx);
    await expect(client.statPath(LEASE, 'a')).rejects.toBeInstanceOf(FileOpUnsupportedError);
    await expect(client.statPath(LEASE, 'a')).rejects.toBeInstanceOf(FileNotFoundError);
    await expect(client.statPath(LEASE, 'a')).rejects.toBeInstanceOf(FileOpFailedError);
    client.close();
  });

  it('uploads a large file as chunked writes at increasing offsets', async () => {
    const writes: Array<{ offset: number; truncate: boolean }> = [];
    onRequest(ctx, (frame, socket) => {
      const op = (frame.payload as { op: Extract<FileOp, { op: 'write' }> }).op;
      writes.push({ offset: op.offset ?? 0, truncate: op.truncate ?? false });
      respond(socket, frame, { outcome: 'written', bytes: 1, size: (op.offset ?? 0) + 1 });
    });
    const client = await connectTestClient(ctx);
    const data = new Uint8Array(FILE_TRANSFER_CHUNK_BYTES * 2 + 10);
    await client.uploadFile(LEASE, 'big.bin', data);
    expect(writes).toEqual([
      { offset: 0, truncate: true },
      { offset: FILE_TRANSFER_CHUNK_BYTES, truncate: false },
      { offset: FILE_TRANSFER_CHUNK_BYTES * 2, truncate: false },
    ]);
    client.close();
  });

  it('pages a download until eof', async () => {
    const pages = [
      {
        outcome: 'content',
        data: bytesToBase64(new Uint8Array([1, 2])),
        offset: 0,
        total_size: 3,
        eof: false,
        truncated: true,
      },
      {
        outcome: 'content',
        data: bytesToBase64(new Uint8Array([3])),
        offset: 2,
        total_size: 3,
        eof: true,
        truncated: false,
      },
    ];
    let n = 0;
    onRequest(ctx, (frame, socket) => {
      respond(socket, frame, pages[n++]);
    });
    const client = await connectTestClient(ctx);
    const bytes = await client.downloadFile(LEASE, 'f');
    expect(Array.from(bytes)).toEqual([1, 2, 3]);
    client.close();
  });
});

describe('parseFileOpResponse', () => {
  it('rejects unknown outcomes and malformed fields', () => {
    expect(() => parseFileOpResponse({ outcome: 'bogus' })).toThrow(/unknown outcome/);
    expect(() => parseFileOpResponse({ outcome: 'written', bytes: 'x', size: 1 })).toThrow(/bad bytes/);
    expect(() => parseFileOpResponse({ outcome: 'failed', kind: 'nope', message: 'm' })).toThrow(/unknown kind/);
    expect(() => parseFileOpResponse(null)).toThrow(/not an object/);
    expect(parseFileOpResponse({ outcome: 'renamed' })).toEqual({ outcome: 'renamed' });
  });
});
