import { afterEach, describe, expect, it } from 'vitest';

import { MEMORY_CAPABILITIES, MemorySandboxClient, SandboxWsClient, leaseId } from '../src/index.ts';
import { connectTestClient, onRequest, respond, respondError, startFakeServer, stopFakeServer } from './harness.ts';
import type { ServerContext } from './harness.ts';

const clients: SandboxWsClient[] = [];
const servers: ServerContext[] = [];

afterEach(async () => {
  for (const client of clients.splice(0)) client.close();
  await Promise.all(servers.splice(0).map(stopFakeServer));
});

async function gateway(result: unknown = { outcome: 'canceled', execution_id: 'owned-execution' }) {
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

describe('command launch control over the WebSocket client', () => {
  it('sends the same bounded lease and launch reference on repeated cancellation', async () => {
    const { client, requests } = await gateway();
    const target = leaseId('owned-lease');
    await expect(client.cancelCommandLaunch(target, 'owned-launch')).resolves.toEqual({
      outcome: 'canceled',
      execution_id: 'owned-execution',
    });
    await client.cancelCommandLaunch(target, 'owned-launch');
    expect(requests).toEqual([
      { tool: 'sandbox_cancel_command_launch', payload: { lease_id: target, launch_key: 'owned-launch' } },
      { tool: 'sandbox_cancel_command_launch', payload: { lease_id: target, launch_key: 'owned-launch' } },
    ]);
  });

  it('opts into durable launch cancellation only when the caller requires it', async () => {
    const { client, requests } = await gateway({ outcome: 'started', execution_id: 'owned', origin: 'created' });
    await client.runCommand(leaseId('owned-lease'), ['serve'], {
      detached: true,
      launchKey: 'owned-launch',
      requireLaunchCancellation: true,
    });
    await client.runCommand(leaseId('owned-lease'), ['serve'], { detached: true, launchKey: 'ordinary-launch' });
    expect(requests[0]).toMatchObject({
      tool: 'sandbox_run_command',
      payload: { launch_key: 'owned-launch', require_launch_cancellation: true },
    });
    expect(requests[1]).toMatchObject({ tool: 'sandbox_run_command', payload: { launch_key: 'ordinary-launch' } });
    expect(requests[1]?.payload).not.toHaveProperty('require_launch_cancellation');
  });

  it.each(['created_elsewhere', null])(
    'refuses malformed provider origin metadata at the public transport seam %#',
    async (origin) => {
      const { client } = await gateway({ outcome: 'started', execution_id: 'owned', origin });
      await expect(client.runCommand(leaseId('owned-lease'), ['serve'], { detached: true })).rejects.toMatchObject({
        code: 'invalid_response',
        outcomeUnknown: true,
      });
    },
  );

  it.each(['', '../foreign', 'spaces are forbidden', 'a'.repeat(129)])(
    'refuses invalid launch reference %# before dispatch',
    async (key) => {
      const { client, requests } = await gateway();
      await expect(client.cancelCommandLaunch(leaseId('owned-lease'), key)).rejects.toMatchObject({
        code: 'invalid_request',
      });
      expect(requests).toEqual([]);
    },
  );

  it('refuses strong foreground or malformed protocol requirements before dispatch', async () => {
    const { client, requests } = await gateway();
    await expect(
      client.runCommand(leaseId('owned-lease'), ['serve'], { requireLaunchCancellation: true }),
    ).rejects.toMatchObject({ code: 'invalid_request' });
    await expect(
      client.runCommand(leaseId('owned-lease'), ['serve'], {
        detached: true,
        requireLaunchCancellation: 'true' as unknown as boolean,
      }),
    ).rejects.toMatchObject({ code: 'invalid_request' });
    expect(requests).toEqual([]);
  });

  it.each([
    { outcome: 'canceled', execution_id: '' },
    { outcome: 'preserved' },
    { outcome: 'pending', message: 'x'.repeat(4097) },
    { outcome: 'canceled', tenant: 'foreign' },
    { outcome: 'denied', kind: 'invalid_spec', message: 'untrusted raw denial' },
    null,
  ])('refuses malformed successful cancellation data %#', async (result) => {
    const { client } = await gateway(result);
    await expect(client.cancelCommandLaunch(leaseId('owned-lease'), 'owned-launch')).rejects.toMatchObject({
      code: 'invalid_response',
      outcomeUnknown: true,
    });
  });

  it('preserves an authenticated gateway denial', async () => {
    const server = await startFakeServer();
    servers.push(server);
    onRequest(server, (frame, socket) => respondError(socket, frame, 'lease_denied', 'launch is outside this lease'));
    const client = await connectTestClient(server);
    clients.push(client);
    await expect(client.cancelCommandLaunch(leaseId('owned-lease'), 'owned-launch')).rejects.toMatchObject({
      code: 'lease_denied',
    });
  });

  it('keeps the memory provider unsupported without inventing durable custody', async () => {
    const client = new MemorySandboxClient();
    expect(MEMORY_CAPABILITIES.unsupported).toContain('command_launch_cancellation');
    await expect(client.cancelCommandLaunch(leaseId('owned-lease'), 'owned-launch')).resolves.toEqual({
      outcome: 'unsupported',
    });
    await expect(
      client.runCommand(leaseId('owned-lease'), ['serve'], { detached: true, requireLaunchCancellation: true }),
    ).rejects.toMatchObject({ code: 'unsupported' });
  });
});
