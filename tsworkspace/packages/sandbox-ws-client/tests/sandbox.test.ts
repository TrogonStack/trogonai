import type { AddressInfo } from 'node:net';

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { WebSocket as NodeWebSocket, WebSocketServer } from 'ws';

import { SandboxWsClient } from '../src/client.ts';
import { digest, leaseId, tenantId } from '../src/ids.ts';
import type { WsFrame } from '../src/protocol.ts';
import { Command, Sandbox } from '../src/sandbox.ts';

interface ServerContext {
  wss: WebSocketServer;
  url: string;
}

function startFakeServer(): Promise<ServerContext> {
  return new Promise((resolve) => {
    const wss = new WebSocketServer({ port: 0, host: '127.0.0.1' });
    wss.on('listening', () => {
      const { port } = wss.address() as AddressInfo;
      resolve({ wss, url: `ws://127.0.0.1:${port}` });
    });
  });
}

function stopFakeServer(ctx: ServerContext): Promise<void> {
  return new Promise((resolve) => {
    ctx.wss.close(() => resolve());
  });
}

function send(socket: NodeWebSocket, frame: WsFrame): void {
  socket.send(JSON.stringify(frame));
}

async function connectTestClient(ctx: ServerContext): Promise<SandboxWsClient> {
  const ClientCtor = SandboxWsClient as unknown as {
    new (socket: WebSocket): SandboxWsClient;
  };
  const socket = new NodeWebSocket(`${ctx.url}/v1/ws`, ['bearer.test-token']) as unknown as WebSocket;
  await new Promise<void>((resolve, reject) => {
    socket.addEventListener('open', () => resolve());
    socket.addEventListener('error', (event: Event) => reject(event));
  });
  return new ClientCtor(socket);
}

describe('SandboxWsClient low-level wrappers', () => {
  let ctx: ServerContext;

  beforeEach(async () => {
    ctx = await startFakeServer();
  });

  afterEach(async () => {
    await stopFakeServer(ctx);
  });

  it('sends a correctly shaped sandbox_stop_lease request frame', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_stop_lease') {
          receivedFrame = frame;
          send(socket, { kind: 'response', id: frame.id, result: { outcome: 'stopped' } });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const result = await client.stopLease(leaseId('lease-stop'));
    expect(result).toEqual({ outcome: 'stopped' });
    expect(receivedFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_stop_lease',
      payload: { lease_id: 'lease-stop' },
    });

    client.close();
  });

  it('sends a correctly shaped sandbox_extend_lease_timeout request frame', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_extend_lease_timeout') {
          receivedFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: { outcome: 'extended', new_deadline: { secs_since_epoch: 2000, nanos_since_epoch: 0 } },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const result = await client.extendLeaseTimeout(leaseId('lease-extend'), 90);
    expect(result).toEqual({
      outcome: 'extended',
      new_deadline: { secs_since_epoch: 2000, nanos_since_epoch: 0 },
    });
    expect(receivedFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_extend_lease_timeout',
      payload: { lease_id: 'lease-extend', additional_seconds: 90 },
    });

    client.close();
  });

  it('sends a correctly shaped sandbox_fork request frame', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_fork') {
          receivedFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: {
              lease_id: 'lease-forked',
              provider: 'mock',
              sandbox_id: 'sandbox-forked',
              evidence_subject: 'evidence.subject',
              deadline: { secs_since_epoch: 3000, nanos_since_epoch: 0 },
            },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const granted = await client.fork(leaseId('lease-source'), 120, 'idem-fork');
    expect(granted.lease_id).toBe('lease-forked');
    expect(receivedFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_fork',
      payload: { source_lease_id: 'lease-source', ttl_seconds: 120, idempotency_key: 'idem-fork' },
    });

    client.close();
  });

  it('sends a correctly shaped sandbox_kill_command request frame', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_kill_command') {
          receivedFrame = frame;
          send(socket, { kind: 'response', id: frame.id, result: { outcome: 'signaled' } });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const result = await client.killCommand(leaseId('lease-kill'), 'exec-1', 'kill');
    expect(result).toEqual({ outcome: 'signaled' });
    expect(receivedFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_kill_command',
      payload: { lease_id: 'lease-kill', execution_id: 'exec-1', signal: 'kill' },
    });

    client.close();
  });

  it('sends the process spec and the named-process verbs', async () => {
    const frames: WsFrame[] = [];
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind !== 'request') {
          return;
        }
        frames.push(frame);
        const results: Record<string, unknown> = {
          sandbox_run_command: { outcome: 'started', execution_id: 'exec-9' },
          sandbox_list_processes: {
            outcome: 'processes',
            processes: [{ execution_id: 'exec-9', name: 'api', state: 'running' }],
          },
          sandbox_wait_ready: { outcome: 'failed', exit_code: 3 },
          sandbox_kill_process: { outcome: 'signaled' },
        };
        send(socket, { kind: 'response', id: frame.id, result: results[frame.tool] });
      });
    });

    const client = await connectTestClient(ctx);
    const lease = leaseId('lease-proc');
    const started = await client.runCommand(lease, ['serve'], {
      detached: true,
      process: { name: 'api', readiness: { kind: 'tcp_port', port: 8080 } },
    });
    expect(started).toEqual({ outcome: 'started', execution_id: 'exec-9' });
    expect(await client.listProcesses(lease)).toEqual({
      outcome: 'processes',
      processes: [{ execution_id: 'exec-9', name: 'api', state: 'running' }],
    });
    expect(await client.waitReady(lease, 'exec-9', 5)).toEqual({ outcome: 'failed', exit_code: 3 });
    expect(await client.killProcess(lease, 'api', 'kill')).toEqual({ outcome: 'signaled' });

    expect(frames.map((f) => (f.kind === 'request' ? f.payload : undefined))).toMatchObject([
      { process: { name: 'api', readiness: { kind: 'tcp_port', port: 8080 } }, detached: true },
      { lease_id: 'lease-proc' },
      { lease_id: 'lease-proc', execution_id: 'exec-9', timeout_seconds: 5 },
      { lease_id: 'lease-proc', name: 'api', signal: 'kill' },
    ]);

    await expect(
      client.runCommand(lease, ['serve'], { detached: true, process: { name: 'bad name' } }),
    ).rejects.toThrow(/process name/);
    client.close();
  });

  it('sends a correctly shaped sandbox_create_drive request frame', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_create_drive') {
          receivedFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: {
              id: { tenant: 'tenant-a', name: 'my-drive' },
              tenant: 'tenant-a',
              size_gib: 100,
              created_at: { secs_since_epoch: 500, nanos_since_epoch: 0 },
              bucket: 'my-bucket',
            },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const record = await client.createDrive('my-drive', 'my-bucket', { sizeGib: 100 });
    expect(record.id.name).toBe('my-drive');
    expect(receivedFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_create_drive',
      payload: { drive_name: 'my-drive', bucket: 'my-bucket', size_gib: 100 },
    });

    client.close();
  });

  it('sends a provider-volume drive as provider_volume instead of bucket', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_create_drive') {
          receivedFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: {
              id: { tenant: 'tenant-a', name: 'data' },
              tenant: 'tenant-a',
              size_gib: 10,
              created_at: { secs_since_epoch: 500, nanos_since_epoch: 0 },
              provider_volume: { provider: 'fly', volume_id: 'vol_example' },
            },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const record = await client.createDrive('data', { provider: 'fly', volume_id: 'vol_example' }, { sizeGib: 10 });
    expect(record.provider_volume?.volume_id).toBe('vol_example');
    expect(receivedFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_create_drive',
      payload: { drive_name: 'data', provider_volume: { provider: 'fly', volume_id: 'vol_example' }, size_gib: 10 },
    });

    client.close();
  });

  it('sends a correctly shaped sandbox_delete_named_sandbox request frame', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_delete_named_sandbox') {
          receivedFrame = frame;
          send(socket, { kind: 'response', id: frame.id, result: { outcome: 'deleted' } });
        }
      });
    });

    const client = await connectTestClient(ctx);
    expect(await client.deleteNamedSandbox('repl-1')).toEqual({ outcome: 'deleted' });
    expect(receivedFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_delete_named_sandbox',
      payload: { name: 'repl-1' },
    });

    client.close();
  });

  it('sends a correctly shaped sandbox_get_domain request frame', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_get_domain') {
          receivedFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: { outcome: 'available', endpoint: 'host:8080' },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const result = await client.getSandboxDomain(leaseId('lease-domain'), 8080);
    expect(result).toEqual({ outcome: 'available', endpoint: 'host:8080' });
    expect(receivedFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_get_domain',
      payload: { lease_id: 'lease-domain', port: 8080 },
    });

    client.close();
  });

  it('sends a correctly shaped sandbox_native_handle request frame', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_native_handle') {
          receivedFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: { outcome: 'available', provider: 'daytona', vendor_id: 'sb-1', region: 'eu' },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const result = await client.getNativeHandle(leaseId('lease-native'));
    expect(result).toEqual({ outcome: 'available', provider: 'daytona', vendor_id: 'sb-1', region: 'eu' });
    expect(receivedFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_native_handle',
      payload: { lease_id: 'lease-native' },
    });

    client.close();
  });

  it('sends a correctly shaped sandbox_ssh_access request frame', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_ssh_access') {
          receivedFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: { outcome: 'available', host: 'ssh.example', port: 22, user: 'tok', expires_at: 1700000000 },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const result = await client.getSshAccess(leaseId('lease-ssh'), 600);
    expect(result).toEqual({
      outcome: 'available',
      host: 'ssh.example',
      port: 22,
      user: 'tok',
      expires_at: 1700000000,
    });
    expect(receivedFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_ssh_access',
      payload: { lease_id: 'lease-ssh', ttl_seconds: 600 },
    });

    client.close();
  });

  it('sends a correctly shaped sandbox_set_snapshot_tag request frame', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_set_snapshot_tag') {
          receivedFrame = frame;
          send(socket, { kind: 'response', id: frame.id, result: { outcome: 'applied' } });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const testDigest = digest(`sha256:${'a'.repeat(64)}`);
    const result = await client.setSnapshotTag('prod', testDigest);
    expect(result).toEqual({ outcome: 'applied' });
    expect(receivedFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_set_snapshot_tag',
      payload: { name: 'prod', digest: testDigest },
    });

    client.close();
  });
});

describe('Sandbox facade', () => {
  let ctx: ServerContext;

  beforeEach(async () => {
    ctx = await startFakeServer();
  });

  afterEach(async () => {
    await stopFakeServer(ctx);
  });

  it('Sandbox.create leases and runCommand(string overload) sends the expected frame', async () => {
    let runFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_lease') {
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: {
              lease_id: 'lease-sbx',
              provider: 'mock',
              sandbox_id: 'sandbox-1',
              evidence_subject: 'evidence.subject',
              deadline: { secs_since_epoch: 1000, nanos_since_epoch: 0 },
            },
          });
        }
        if (frame.kind === 'request' && frame.tool === 'sandbox_run_command') {
          runFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: { outcome: 'completed', exit_code: 0, stdout: [104, 105], stderr: [] },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const sandbox = await Sandbox.create(client, { tenant: tenantId('tenant-a') });
    expect(sandbox.sandboxId).toBe('lease-sbx');
    expect(sandbox.provider).toBe('mock');

    const result = await sandbox.runCommand('node', ['--version']);
    expect(result.exitCode).toBe(0);
    expect(result.stdout()).toBe('hi');
    expect(runFrame).toMatchObject({
      tool: 'sandbox_run_command',
      payload: { lease_id: 'lease-sbx', command: ['node', '--version'] },
    });

    client.close();
  });

  it('detached runCommand returns a Command, and command.kill sends the signal frame', async () => {
    let killFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_lease') {
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: {
              lease_id: 'lease-detach',
              provider: 'mock',
              sandbox_id: 'sandbox-2',
              evidence_subject: 'evidence.subject',
              deadline: { secs_since_epoch: 1000, nanos_since_epoch: 0 },
            },
          });
        }
        if (frame.kind === 'request' && frame.tool === 'sandbox_run_command') {
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: { outcome: 'started', execution_id: 'exec-42' },
          });
        }
        if (frame.kind === 'request' && frame.tool === 'sandbox_kill_command') {
          killFrame = frame;
          send(socket, { kind: 'response', id: frame.id, result: { outcome: 'signaled' } });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const sandbox = await Sandbox.create(client, { tenant: tenantId('tenant-a') });
    const command = await sandbox.runCommand({ cmd: 'sleep', args: ['5'], detached: true });
    expect(command).toBeInstanceOf(Command);
    expect(command.cmdId).toBe('exec-42');

    await command.kill();
    expect(killFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_kill_command',
      payload: { lease_id: 'lease-detach', execution_id: 'exec-42', signal: 'term' },
    });

    client.close();
  });

  it('command.wait polls sandbox_poll_command until the response is completed', async () => {
    let pollCount = 0;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_lease') {
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: {
              lease_id: 'lease-wait',
              provider: 'mock',
              sandbox_id: 'sandbox-3',
              evidence_subject: 'evidence.subject',
              deadline: { secs_since_epoch: 1000, nanos_since_epoch: 0 },
            },
          });
        }
        if (frame.kind === 'request' && frame.tool === 'sandbox_run_command') {
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: { outcome: 'started', execution_id: 'exec-wait' },
          });
        }
        if (frame.kind === 'request' && frame.tool === 'sandbox_poll_command') {
          pollCount += 1;
          if (pollCount < 3) {
            send(socket, { kind: 'response', id: frame.id, result: { outcome: 'running' } });
          } else {
            send(socket, {
              kind: 'response',
              id: frame.id,
              result: { outcome: 'completed', exit_code: 0, stdout: [111, 107], stderr: [] },
            });
          }
        }
      });
    });

    const client = await connectTestClient(ctx);
    const sandbox = await Sandbox.create(client, { tenant: tenantId('tenant-a') });
    const command = await sandbox.runCommand({ cmd: 'sleep', args: ['1'], detached: true });
    const finished = await command.wait({ pollIntervalMs: 1 });
    expect(finished.exitCode).toBe(0);
    expect(finished.stdout()).toBe('ok');
    expect(pollCount).toBe(3);

    client.close();
  });

  it('mkDir sends a mkdir -p frame', async () => {
    let runFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_lease') {
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: {
              lease_id: 'lease-mkdir',
              provider: 'mock',
              sandbox_id: 'sandbox-4',
              evidence_subject: 'evidence.subject',
              deadline: { secs_since_epoch: 1000, nanos_since_epoch: 0 },
            },
          });
        }
        if (frame.kind === 'request' && frame.tool === 'sandbox_run_command') {
          runFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: { outcome: 'completed', exit_code: 0, stdout: [], stderr: [] },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const sandbox = await Sandbox.create(client, { tenant: tenantId('tenant-a') });
    await sandbox.mkDir('/tmp/assets');

    expect(runFrame).toMatchObject({
      tool: 'sandbox_run_command',
      payload: { lease_id: 'lease-mkdir', command: ['mkdir', '-p', '/tmp/assets'] },
    });

    client.close();
  });

  it('extendTimeout converts milliseconds to whole seconds and returns the new deadline', async () => {
    let extendFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_lease') {
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: {
              lease_id: 'lease-timeout',
              provider: 'mock',
              sandbox_id: 'sandbox-5',
              evidence_subject: 'evidence.subject',
              deadline: { secs_since_epoch: 1000, nanos_since_epoch: 0 },
            },
          });
        }
        if (frame.kind === 'request' && frame.tool === 'sandbox_extend_lease_timeout') {
          extendFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: { outcome: 'extended', new_deadline: { secs_since_epoch: 1090, nanos_since_epoch: 0 } },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const sandbox = await Sandbox.create(client, { tenant: tenantId('tenant-a') });
    const newDeadline = await sandbox.extendTimeout(90_500);

    expect(extendFrame).toMatchObject({
      tool: 'sandbox_extend_lease_timeout',
      payload: { lease_id: 'lease-timeout', additional_seconds: 91 },
    });
    expect(newDeadline).toEqual(new Date(1090 * 1000));
    expect(sandbox.deadline).toEqual(new Date(1090 * 1000));

    client.close();
  });

  it('Sandbox.get looks up a lease by name via sandbox_get_lease_by_name', async () => {
    let receivedFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_get_lease_by_name') {
          receivedFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: {
              outcome: 'found',
              row: {
                lease_id: 'lease-named',
                tenant: 'tenant-a',
                provider: 'mock',
                status: 'running',
                created_at: { secs_since_epoch: 1000, nanos_since_epoch: 0 },
                deadline: { secs_since_epoch: 1300, nanos_since_epoch: 0 },
                name: 'my-sandbox',
              },
            },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const sandbox = await Sandbox.get(client, { name: 'my-sandbox' });
    expect(sandbox.sandboxId).toBe('lease-named');
    expect(sandbox.provider).toBe('mock');
    expect(sandbox.name).toBe('my-sandbox');
    expect(sandbox.deadline).toEqual(new Date(1300 * 1000));
    expect(receivedFrame).toEqual({
      kind: 'request',
      id: expect.any(String),
      tool: 'sandbox_get_lease_by_name',
      payload: { name: 'my-sandbox' },
    });

    client.close();
  });

  it('Sandbox.get throws when sandbox_get_lease_by_name returns not_found', async () => {
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_get_lease_by_name') {
          send(socket, { kind: 'response', id: frame.id, result: { outcome: 'not_found' } });
        }
      });
    });

    const client = await connectTestClient(ctx);
    await expect(Sandbox.get(client, { name: 'missing' })).rejects.toThrow(/not found/);

    client.close();
  });

  it('fs.exists maps to a `test -e` run_command frame and returns a boolean from the exit code', async () => {
    let runFrame: WsFrame | undefined;
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_lease') {
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: {
              lease_id: 'lease-fs',
              provider: 'mock',
              sandbox_id: 'sandbox-fs',
              evidence_subject: 'evidence.subject',
              deadline: { secs_since_epoch: 1000, nanos_since_epoch: 0 },
            },
          });
        }
        if (frame.kind === 'request' && frame.tool === 'sandbox_run_command') {
          runFrame = frame;
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: { outcome: 'completed', exit_code: 1, stdout: [], stderr: [] },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const sandbox = await Sandbox.create(client, { tenant: tenantId('tenant-a') });
    const exists = await sandbox.fs.exists('/tmp/missing');

    expect(exists).toBe(false);
    expect(runFrame).toMatchObject({
      tool: 'sandbox_run_command',
      payload: { lease_id: 'lease-fs', command: ['test', '-e', '/tmp/missing'] },
    });

    client.close();
  });

  it('Command.stream decodes CommandOutputChunk evidence events for this execution id', async () => {
    ctx.wss.on('connection', (socket) => {
      socket.on('message', (raw) => {
        const frame = JSON.parse(String(raw)) as WsFrame;
        if (frame.kind === 'request' && frame.tool === 'sandbox_lease') {
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: {
              lease_id: 'lease-stream',
              provider: 'mock',
              sandbox_id: 'sandbox-stream',
              evidence_subject: 'evidence.subject',
              deadline: { secs_since_epoch: 1000, nanos_since_epoch: 0 },
            },
          });
        }
        if (frame.kind === 'request' && frame.tool === 'sandbox_run_command') {
          send(socket, {
            kind: 'response',
            id: frame.id,
            result: { outcome: 'started', execution_id: 'exec-stream' },
          });
        }
        if (frame.kind === 'request' && frame.tool === 'sandbox_subscribe_evidence') {
          send(socket, { kind: 'response', id: frame.id, result: { subscribed: true } });
          const chunk = { execution_id: 'exec-stream', stream: 'stdout', seq: 0, data: [104, 105] };
          send(socket, {
            kind: 'event',
            stream_id: 'lease-stream',
            event: { subject_suffix: 'command.exec-stream', payload: JSON.stringify(chunk) },
          });
          // Unrelated evidence for a different execution id, filtered out by Command.stream.
          send(socket, {
            kind: 'event',
            stream_id: 'lease-stream',
            event: {
              subject_suffix: 'command.other-exec',
              payload: JSON.stringify({ ...chunk, execution_id: 'other-exec' }),
            },
          });
        }
      });
    });

    const client = await connectTestClient(ctx);
    const sandbox = await Sandbox.create(client, { tenant: tenantId('tenant-a') });
    const command = await sandbox.runCommand({ cmd: 'sleep', args: ['5'], detached: true, streamOutput: true });
    expect(command).toBeInstanceOf(Command);

    const iterator = (command as Command).stream()[Symbol.asyncIterator]();
    const first = await iterator.next();
    expect(first.done).toBe(false);
    expect(first.value).toEqual({ stream: 'stdout', data: Uint8Array.from([104, 105]) });

    client.close();
  });
});
