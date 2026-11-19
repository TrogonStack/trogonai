import type { AddressInfo } from 'node:net';

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { WebSocket as NodeWebSocket, WebSocketServer } from 'ws';

import type { SandboxClientApi } from '../src/client-api.ts';
import { SandboxWsClient } from '../src/client.ts';
import { leaseId, tenantId } from '../src/ids.ts';
import { MEMORY_CAPABILITIES, MemorySandboxClient } from '../src/memory.ts';
import type { WsFrame } from '../src/protocol.ts';
import { Command, Sandbox } from '../src/sandbox.ts';

async function connect(wss: WebSocketServer): Promise<SandboxWsClient> {
  const { port } = wss.address() as AddressInfo;
  const ClientCtor = SandboxWsClient as unknown as { new (socket: WebSocket): SandboxWsClient };
  const socket = new NodeWebSocket(`ws://127.0.0.1:${port}/v1/ws`, ['bearer.test-token']) as unknown as WebSocket;
  await new Promise<void>((resolve, reject) => {
    socket.addEventListener('open', () => resolve());
    socket.addEventListener('error', (event: Event) => reject(event));
  });
  return new ClientCtor(socket);
}

describe('stop, restart and durable logs over a fake gateway', () => {
  let wss: WebSocketServer;
  const seen: { tool: string; payload: Record<string, unknown> }[] = [];

  beforeEach(async () => {
    seen.length = 0;
    wss = await new Promise<WebSocketServer>((resolve) => {
      const server = new WebSocketServer({ port: 0, host: '127.0.0.1' });
      server.on('listening', () => resolve(server));
    });
    wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind !== 'request') return;
        seen.push({ tool: frame.tool, payload: frame.payload as Record<string, unknown> });
        const results: Record<string, unknown> = {
          sandbox_stop_command: { outcome: 'stopped', signal: 'kill' },
          sandbox_restart_command: { outcome: 'restarted', execution_id: 'exec-2' },
          sandbox_read_logs: {
            outcome: 'logs',
            chunks: [{ log_seq: 7, stream: 'stderr', seq: 0, data: [104, 105] }],
            next_seq: 7,
            has_more: true,
            truncated: false,
          },
        };
        socket.send(JSON.stringify({ kind: 'response', id: frame.id, result: results[frame.tool] }));
      });
    });
  });

  afterEach(async () => {
    await new Promise<void>((resolve) => wss.close(() => resolve()));
  });

  it('sends the three verbs with their wire field names', async () => {
    const client = await connect(wss);
    const lease = leaseId('lease-1');
    expect(await client.stopCommand(lease, 'exec-1', 3)).toEqual({ outcome: 'stopped', signal: 'kill' });
    expect(await client.restartCommand(lease, 'exec-1')).toEqual({ outcome: 'restarted', execution_id: 'exec-2' });
    const page = await client.readLogs(lease, 'exec-1', { afterSeq: 4, maxBytes: 100 });
    expect(page.outcome).toBe('logs');
    expect(seen.map((s) => s.tool)).toEqual(['sandbox_stop_command', 'sandbox_restart_command', 'sandbox_read_logs']);
    expect(seen[0].payload).toMatchObject({ lease_id: 'lease-1', execution_id: 'exec-1', grace_seconds: 3 });
    expect(seen[2].payload).toMatchObject({ after_seq: 4, max_bytes: 100 });
    client.close();
  });

  it('maps Command.stop, restart and logPage onto the client', async () => {
    const client = await connect(wss);
    const command = new Command(client as unknown as SandboxClientApi, leaseId('lease-1'), 'exec-1');
    expect(await command.stop({ graceMs: 2500 })).toEqual({ outcome: 'stopped', signal: 'kill' });
    expect(seen[0].payload.grace_seconds).toBe(3);
    const restarted = await command.restart();
    expect(restarted.cmdId).toBe('exec-2');
    const page = await command.logPage({ after: 0 });
    expect(page.nextSeq).toBe(7);
    expect(page.hasMore).toBe(true);
    expect(page.exited).toBe(false);
    expect(page.chunks[0].stream).toBe('stderr');
    expect(new TextDecoder().decode(page.chunks[0].data)).toBe('hi');
    client.close();
  });
});

describe('stop, restart and logs on the in-memory client', () => {
  it('stops and restarts a running command with term while logs remain unsupported', async () => {
    const client = new MemorySandboxClient({
      commands: {
        hang: ({ signal }) =>
          new Promise((resolve) => signal.addEventListener('abort', () => resolve({ exitCode: 0 }))),
      },
    });
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme'), timeoutMs: 60_000 });
    const cmd = await sandbox.runCommand({ cmd: 'hang', detached: true });
    expect(await cmd.stop({ graceMs: 1000 })).toEqual({ outcome: 'stopped', signal: 'term' });
    expect((await cmd.wait({ pollIntervalMs: 1 })).exitCode).toBe(143);
    expect(await client.stopCommand(sandbox.sandboxId, cmd.cmdId)).toEqual({ outcome: 'already_completed' });
    expect(await client.stopCommand(sandbox.sandboxId, 'missing')).toEqual({ outcome: 'not_found' });
    const restarted = await cmd.restart();
    expect(restarted.cmdId).not.toBe(cmd.cmdId);
    expect(await client.pollCommand(sandbox.sandboxId, restarted.cmdId)).toEqual({ outcome: 'running' });
    await restarted.stop();
    await expect(cmd.logPage()).rejects.toMatchObject({ code: 'unsupported' });
    expect(MEMORY_CAPABILITIES.supported).toContain('stop_command');
    expect(MEMORY_CAPABILITIES.supported).toContain('restart_command');
    expect(MEMORY_CAPABILITIES.unsupported).toContain('read_logs');
  });
});
