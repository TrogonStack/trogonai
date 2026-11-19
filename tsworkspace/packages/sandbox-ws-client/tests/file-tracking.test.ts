import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { FILE_OP_ERROR_CLASSES } from '../src/error-classes.ts';
import { FileOpFailedError, FileOpUnsupportedError, parseFileOpResponse } from '../src/fileops.ts';
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

const LEASE = leaseId('lease-tracking');

function opOf(frame: { payload: unknown }): FileOp {
  return (frame.payload as { op: FileOp }).op;
}

describe('change tracking and file watch', () => {
  let ctx: ServerContext;

  beforeEach(async () => {
    ctx = await startFakeServer();
  });

  afterEach(async () => {
    await stopFakeServer(ctx);
  });

  it('checkpoints a tree and reads the net changes since it', async () => {
    const seen: FileOp[] = [];
    onRequest(ctx, (frame, socket) => {
      const op = opOf(frame);
      seen.push(op);
      if (op.op === 'checkpoint') {
        respond(socket, frame, { outcome: 'checkpointed', checkpoint_id: 'cp-1', entries: 4 });
      } else {
        respond(socket, frame, {
          outcome: 'changes',
          since: 'cp-1',
          added: ['/w/new'],
          modified: ['/w/a'],
          deleted: ['/w/gone'],
          truncated: false,
        });
      }
    });
    const client = await connectTestClient(ctx);
    const cp = await client.checkpointFiles(LEASE, '/w', { hash: true });
    expect(cp).toEqual({ checkpointId: 'cp-1', entries: 4 });
    const changes = await client.fileChanges(LEASE, { since: cp.checkpointId });
    expect(changes.added).toEqual(['/w/new']);
    expect(changes.deleted).toEqual(['/w/gone']);
    expect(seen[0]).toMatchObject({ op: 'checkpoint', path: '/w', hash: true });
    expect(seen[1]).toMatchObject({ op: 'changes', since: 'cp-1' });
    client.close();
  });

  it('turns unsupported and unknown-checkpoint replies into typed errors', async () => {
    const replies = [
      { outcome: 'unsupported', message: 'no tracking here' },
      { outcome: 'failed', kind: 'unknown_checkpoint', message: 'cp-9' },
    ];
    let n = 0;
    onRequest(ctx, (frame, socket) => {
      respond(socket, frame, replies[n++]);
    });
    const client = await connectTestClient(ctx);
    await expect(client.fileChanges(LEASE)).rejects.toBeInstanceOf(FileOpUnsupportedError);
    const err = await client.fileChanges(LEASE, { since: 'cp-9' }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(FileOpFailedError);
    expect((err as FileOpFailedError).kind).toBe('unknown_checkpoint');
    client.close();
  });

  it('streams watch events across polls with a moving cursor, then stops the watch', async () => {
    const polls: number[] = [];
    let stopped = false;
    onRequest(ctx, (frame, socket) => {
      const op = opOf(frame);
      if (op.op === 'watch_start') {
        respond(socket, frame, { outcome: 'watch_started', watch_id: 'w-1' });
      } else if (op.op === 'watch_poll') {
        polls.push(op.after_seq ?? 0);
        if (polls.length === 1) {
          respond(socket, frame, {
            outcome: 'watch_events',
            events: [{ seq: 1, kind: 'created', path: '/w/a', is_dir: false }],
            next_seq: 1,
            overflowed: false,
          });
        } else {
          respond(socket, frame, {
            outcome: 'watch_events',
            events: [{ seq: 2, kind: 'deleted', path: '/w/a', is_dir: false }],
            next_seq: 2,
            overflowed: true,
          });
        }
      } else {
        stopped = true;
        respond(socket, frame, { outcome: 'watch_stopped', existed: true });
      }
    });
    const client = await connectTestClient(ctx);
    const items = [];
    for await (const item of client.watchFiles(LEASE, '/w', { recursive: true })) {
      items.push(item);
      if (items.length === 3) break;
    }
    expect(items).toEqual([
      { type: 'event', event: { seq: 1, kind: 'created', path: '/w/a', is_dir: false } },
      { type: 'overflowed' },
      { type: 'event', event: { seq: 2, kind: 'deleted', path: '/w/a', is_dir: false } },
    ]);
    expect(polls).toEqual([0, 1]);
    expect(stopped).toBe(true);
    client.close();
  });

  it('throws FileOpUnsupportedError from the first iteration when the provider cannot watch', async () => {
    onRequest(ctx, (frame, socket) => {
      respond(socket, frame, { outcome: 'unsupported', message: 'no watch' });
    });
    const client = await connectTestClient(ctx);
    const iterate = async () => {
      for await (const _ of client.watchFiles(LEASE, '/w')) {
        // never reached
      }
    };
    await expect(iterate()).rejects.toBeInstanceOf(FileOpUnsupportedError);
    client.close();
  });

  it('validates the new reply shapes and classifies the new error kinds', () => {
    expect(() => parseFileOpResponse({ outcome: 'changes', since: 's' })).toThrow(/bad added/);
    expect(parseFileOpResponse({ outcome: 'watch_stopped', existed: false })).toEqual({
      outcome: 'watch_stopped',
      existed: false,
    });
    expect(() => parseFileOpResponse({ outcome: 'failed', kind: 'unknown_watch', message: 'm' })).not.toThrow();
    expect(FILE_OP_ERROR_CLASSES.unknown_checkpoint?.retryable).toBe(false);
    expect(FILE_OP_ERROR_CLASSES.unknown_watch?.retryable).toBe(false);
  });
});
