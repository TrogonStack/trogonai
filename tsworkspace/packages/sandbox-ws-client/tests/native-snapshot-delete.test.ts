import { afterEach, describe, expect, it } from 'vitest';

import { MEMORY_CAPABILITIES, MemorySandboxClient, Sandbox } from '../src/index.ts';
import type { NativeSnapshotRef, SandboxWsClient } from '../src/index.ts';
import { connectTestClient, onRequest, respond, respondError, startFakeServer, stopFakeServer } from './harness.ts';
import type { ServerContext } from './harness.ts';

const reference: NativeSnapshotRef = { provider: 'e2b', id: '01JABC1234', expires_at: 0 };
const clients: SandboxWsClient[] = [];
const servers: ServerContext[] = [];

afterEach(async () => {
  for (const client of clients.splice(0)) client.close();
  await Promise.all(servers.splice(0).map(stopFakeServer));
});

async function gateway(result: unknown = { outcome: 'deleted' }) {
  const server = await startFakeServer();
  servers.push(server);
  const requests: Array<{ tool: string; payload: unknown }> = [];
  onRequest(server, (frame, socket) => {
    requests.push({ tool: frame.tool, payload: frame.payload });
    respond(socket, frame, result);
  });
  const client = await connectTestClient(server);
  clients.push(client);
  return { client, requests };
}

describe('provider-native snapshot deletion', () => {
  it('sends only the opaque provider reference through the authenticated gateway', async () => {
    const { client, requests } = await gateway();
    await expect(client.nativeSnapshotDelete(reference)).resolves.toEqual({ outcome: 'deleted' });
    expect(requests).toEqual([{ tool: 'sandbox_native_snapshot_delete', payload: { reference } }]);
  });

  it('allows an expired reference to retire its owned provider artifact', async () => {
    const { client } = await gateway();
    await expect(Sandbox.deleteNativeSnapshot(client, reference)).resolves.toBe(true);
  });

  it('reports a confirmed repeat as false', async () => {
    const { client } = await gateway({ outcome: 'not_found' });
    await expect(Sandbox.deleteNativeSnapshot(client, reference)).resolves.toBe(false);
  });

  it('rejects malformed references before dispatch', async () => {
    const { client, requests } = await gateway();
    for (const invalid of [
      { provider: 'unknown', id: 'snapshot' },
      { provider: 'e2b', id: '' },
      { provider: 'e2b', id: '../foreign' },
      { provider: 'e2b', id: 'raw:default' },
      { provider: 'e2b', id: 'a'.repeat(129) },
      { provider: 'e2b', id: 'snapshot', expires_at: -1 },
      { provider: 'e2b', id: 'snapshot', expires_at: Number.MAX_SAFE_INTEGER + 1 },
      { provider: 'e2b', id: 'snapshot', tenant: 'foreign' },
    ]) {
      await expect(client.nativeSnapshotDelete(invalid as NativeSnapshotRef)).rejects.toMatchObject({
        code: 'invalid_request',
      });
    }
    expect(requests).toEqual([]);
  });

  it.each([
    { outcome: 'queued' },
    { outcome: 'deleted', bytes: 12 },
    { outcome: 'denied', kind: 'invalid_spec', message: 'untrusted raw denial' },
    null,
  ])('refuses malformed successful response data %#', async (result) => {
    const { client } = await gateway(result);
    await expect(client.nativeSnapshotDelete(reference)).rejects.toMatchObject({
      code: 'invalid_response',
      outcomeUnknown: true,
    });
  });

  it('preserves typed gateway denial rather than claiming deletion', async () => {
    const server = await startFakeServer();
    servers.push(server);
    onRequest(server, (frame, socket) =>
      respondError(socket, frame, 'lease_denied', 'snapshot does not belong to this tenant'),
    );
    const client = await connectTestClient(server);
    clients.push(client);
    await expect(Sandbox.deleteNativeSnapshot(client, reference)).rejects.toMatchObject({ code: 'lease_denied' });
  });

  it('refuses providers without native deletion support', async () => {
    const { client } = await gateway({ outcome: 'unsupported' });
    await expect(client.nativeSnapshotDelete(reference)).resolves.toEqual({ outcome: 'unsupported' });
    await expect(Sandbox.deleteNativeSnapshot(client, reference)).rejects.toMatchObject({ code: 'unsupported' });
  });

  it('declares the memory provider unsupported without inventing snapshot custody', async () => {
    const client = new MemorySandboxClient();
    expect(MEMORY_CAPABILITIES.unsupported).toContain('native_snapshot_delete');
    await expect(client.nativeSnapshotDelete(reference)).resolves.toEqual({ outcome: 'unsupported' });
    await expect(Sandbox.deleteNativeSnapshot(client, reference)).rejects.toMatchObject({ code: 'unsupported' });
    await expect(client.nativeSnapshotDelete({ ...reference, id: 'foreign/template' })).rejects.toMatchObject({
      code: 'invalid_request',
    });
  });
});
