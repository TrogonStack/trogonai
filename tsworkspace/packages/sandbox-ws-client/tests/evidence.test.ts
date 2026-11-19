import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { leaseId } from '../src/ids.ts';
import {
  connectTestClient,
  onRequest,
  respond,
  send,
  startFakeServer,
  stopFakeServer,
  type ServerContext,
} from './harness.ts';

describe('evidence subscriptions', () => {
  let ctx: ServerContext;
  let subscribeCount: number;

  beforeEach(async () => {
    ctx = await startFakeServer();
    subscribeCount = 0;
    onRequest(ctx, (frame, socket) => {
      if (frame.tool === 'sandbox_subscribe_evidence') {
        subscribeCount += 1;
        respond(socket, frame, { subscribed: true });
        send(socket, {
          kind: 'event',
          stream_id: 'lease-e',
          event: { subject_suffix: 'command.x', payload: 'hello' },
        });
      }
    });
  });

  afterEach(async () => {
    await stopFakeServer(ctx);
  });

  it('fans one gateway subscription out to every listener', async () => {
    const client = await connectTestClient(ctx);
    const a = client.subscribeEvidence(leaseId('lease-e'));
    const b = client.subscribeEvidence(leaseId('lease-e'));
    await Promise.all([a.ready, b.ready]);
    const first = await a.events[Symbol.asyncIterator]().next();
    const second = await b.events[Symbol.asyncIterator]().next();
    expect(first.value).toEqual({ subjectSuffix: 'command.x', payload: 'hello' });
    expect(second.value).toEqual(first.value);
    expect(subscribeCount).toBe(1);
    client.close();
  });

  it('keeps evidenceStream working alongside a subscription and ends listeners on close', async () => {
    const client = await connectTestClient(ctx);
    const sub = client.subscribeEvidence(leaseId('lease-e'));
    const iterator = client.evidenceStream(leaseId('lease-e'))[Symbol.asyncIterator]();
    expect((await iterator.next()).value).toEqual({ subjectSuffix: 'command.x', payload: 'hello' });
    const subIterator = sub.events[Symbol.asyncIterator]();
    expect((await subIterator.next()).value).toEqual({ subjectSuffix: 'command.x', payload: 'hello' });
    sub.close();
    expect((await subIterator.next()).done).toBe(true);
    expect(subscribeCount).toBe(1);
    client.close();
    expect((await iterator.next()).done).toBe(true);
  });

  it('rejects ready and allows a later retry when the gateway refuses the subscription', async () => {
    const refusing = await startFakeServer();
    let calls = 0;
    onRequest(refusing, (frame, socket) => {
      calls += 1;
      if (calls === 1) {
        send(socket, { kind: 'response', id: frame.id, error: { code: 'lease_denied', message: 'no' } });
      } else {
        respond(socket, frame, { subscribed: true });
      }
    });
    const client = await connectTestClient(refusing);
    await expect(client.subscribeEvidence(leaseId('l')).ready).rejects.toMatchObject({ code: 'lease_denied' });
    await expect(client.subscribeEvidence(leaseId('l')).ready).resolves.toBeUndefined();
    client.close();
    await stopFakeServer(refusing);
  });
});
