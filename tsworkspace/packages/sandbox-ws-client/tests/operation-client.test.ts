import { afterEach, describe, expect, it } from 'vitest';

import type { SandboxWsClient } from '../src/index.ts';
import { connectTestClient, onRequest, respond, startFakeServer, stopFakeServer } from './harness.ts';
import type { ServerContext } from './harness.ts';

const clients: SandboxWsClient[] = [];
const servers: ServerContext[] = [];
afterEach(async () => {
  for (const client of clients.splice(0)) client.close();
  await Promise.all(servers.splice(0).map(stopFakeServer));
});
async function gateway(response: unknown) {
  const server = await startFakeServer();
  servers.push(server);
  const requests: Array<{ tool: string; payload: unknown }> = [];
  onRequest(server, (frame, socket) => {
    requests.push({ tool: frame.tool, payload: frame.payload });
    respond(socket, frame, response);
  });
  const client = await connectTestClient(server);
  clients.push(client);
  return { client, requests };
}

describe('operation control over the authenticated gateway', () => {
  it('asks for create cancellation support before selecting any provider', async () => {
    const { client, requests } = await gateway({ supports_create_cancellation: true });
    await expect(client.operationCapabilities()).resolves.toEqual({ supports_create_cancellation: true });
    expect(requests).toEqual([{ tool: 'sandbox_operation_capabilities', payload: {} }]);
  });

  it('cancels only the operation id without accepting a tenant override', async () => {
    const { client, requests } = await gateway({ outcome: 'canceled' });
    await expect(client.cancelOperation('op-1')).resolves.toEqual({ outcome: 'canceled' });
    expect(requests).toEqual([{ tool: 'sandbox_cancel_operation', payload: { operation_id: 'op-1' } }]);
  });

  it('explicitly opts into broker-owned compensation without changing plain cancellation', async () => {
    const { client, requests } = await gateway({ outcome: 'canceled' });
    await expect(client.cancelOperation('op-1', { compensateCreated: true })).resolves.toEqual({ outcome: 'canceled' });
    expect(requests).toEqual([
      { tool: 'sandbox_cancel_operation', payload: { operation_id: 'op-1', compensate_created: true } },
    ]);
  });

  it('looks up a tenant-owned operation status', async () => {
    const { client, requests } = await gateway({
      outcome: 'found',
      operation: { id: 'op-1', tenant: 'acme', kind: 'create', state: 'running' },
    });
    await expect(client.getOperation('op-1')).resolves.toMatchObject({
      outcome: 'found',
      operation: { id: 'op-1', state: 'running' },
    });
    expect(requests).toEqual([{ tool: 'sandbox_get_operation', payload: { operation_id: 'op-1' } }]);
  });

  it.each(['pending', 'cleaned'] as const)(
    'retains validated compensation state %s without changing the original operation state',
    async (compensation) => {
      const { client } = await gateway({
        outcome: 'found',
        operation: { id: 'op-1', tenant: 'acme', kind: 'create', state: 'succeeded', compensation },
      });
      await expect(client.getOperation('op-1')).resolves.toMatchObject({
        outcome: 'found',
        operation: { id: 'op-1', state: 'succeeded', compensation },
      });
    },
  );

  it('refuses an unknown compensation state rather than hiding it', async () => {
    const { client } = await gateway({
      outcome: 'found',
      operation: { id: 'op-1', tenant: 'acme', kind: 'create', state: 'succeeded', compensation: 'assumed_deleted' },
    });
    await expect(client.getOperation('op-1')).rejects.toMatchObject({ code: 'invalid_response' });
  });

  it('refuses malformed operation ids before any control request', async () => {
    const { client, requests } = await gateway({ outcome: 'canceled' });
    for (const id of ['', '../other', 'a'.repeat(129)]) {
      await expect(client.cancelOperation(id)).rejects.toMatchObject({ code: 'invalid_request' });
      await expect(client.getOperation(id)).rejects.toMatchObject({ code: 'invalid_request' });
    }
    expect(requests).toEqual([]);
  });

  it('rejects malformed control response data without claiming cancellation', async () => {
    const { client } = await gateway({ outcome: 'already_completed', state: 'unknown' });
    await expect(client.cancelOperation('op-1')).rejects.toMatchObject({
      code: 'invalid_response',
      outcomeUnknown: true,
    });
    await expect(client.operationCapabilities()).rejects.toMatchObject({ code: 'invalid_response' });
  });
});
