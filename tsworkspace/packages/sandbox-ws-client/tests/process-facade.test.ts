import { afterEach, describe, expect, it, vi } from 'vitest';

import { MemorySandboxClient, Sandbox, ServerExitedError, tenantId } from '../src/index.ts';
import type { ListProcessesResponse } from '../src/index.ts';

const tenant = tenantId('acme');
const sandboxes: Sandbox[] = [];
const endings: Array<() => void> = [];
afterEach(async () => {
  for (const sandbox of sandboxes.splice(0)) await sandbox.delete();
  for (const end of endings.splice(0)) end();
});

async function fixture() {
  const launches: Array<{ cwd: string; env: Record<string, string> }> = [];
  const client = new MemorySandboxClient({
    commands: {
      work: ({ signal, cwd, env }) => {
        launches.push({ cwd, env });
        return new Promise((resolve) => {
          const end = () => resolve({ exitCode: 143 });
          endings.push(end);
          signal.addEventListener('abort', end, { once: true });
        });
      },
    },
  });
  const sandbox = await Sandbox.create(client, { tenant });
  sandboxes.push(sandbox);
  return { client, sandbox, launches };
}

describe('named process facade', () => {
  it('gets or starts a named execution and discovers it from a fresh handle without launching a duplicate', async () => {
    const { client, sandbox, launches } = await fixture();
    const original = await sandbox.start({ cmd: 'work', name: 'worker', cwd: '/work', env: { MODE: 'queue' } });
    const repeated = await sandbox.start({ cmd: 'work', name: 'worker' });
    expect(repeated.command.cmdId).toBe(original.command.cmdId);
    expect(launches).toHaveLength(1);
    const attached = await Sandbox.connect(client, sandbox.sandboxId);
    const found = await attached.process('worker');
    expect(found?.name).toBe('worker');
    expect(found?.command.cmdId).toBe(original.command.cmdId);
    expect(await attached.processes()).toContainEqual({
      execution_id: original.command.cmdId,
      name: 'worker',
      state: 'running',
    });
    await found!.stop({ graceMs: 0 });
    expect((await original.wait()).exitCode).toBe(143);
    expect(await attached.process('absent')).toBeNull();
  });

  it('uses a stable valid default name across client handles', async () => {
    const { client, sandbox, launches } = await fixture();
    const first = await sandbox.start({ cmd: 'work', args: ['--queue'] });
    const attached = await Sandbox.connect(client, sandbox.sandboxId);
    const second = await attached.start({ cmd: 'work', args: ['--queue'] });
    expect(first.name).toMatch(/^[A-Za-z0-9._-]{1,64}$/);
    expect(second.name).toBe(first.name);
    expect(second.command.cmdId).toBe(first.command.cmdId);
    expect(launches).toHaveLength(1);
  });

  it('restarts a discovered process through its recorded backend specification', async () => {
    const { client, sandbox, launches } = await fixture();
    const first = await sandbox.start({ cmd: 'work', name: 'worker', cwd: '/work', env: { MODE: 'queue' } });
    const attached = await Sandbox.connect(client, sandbox.sandboxId);
    const discovered = (await attached.process('worker'))!;
    await discovered.restart({ graceMs: 0 });
    expect(discovered.command.cmdId).not.toBe(first.command.cmdId);
    expect(launches).toHaveLength(2);
    expect(launches[1]).toMatchObject({ cwd: '/work', env: { MODE: 'queue' } });
  });

  it('forwards named readiness to the worker and waits for the reported execution', async () => {
    const { client, sandbox } = await fixture();
    const run = vi.spyOn(client, 'runCommand').mockResolvedValue({ outcome: 'started', execution_id: 'ready-exec' });
    const wait = vi.spyOn(client, 'waitReady').mockResolvedValue({ outcome: 'ready' });
    const process = await sandbox.start({
      cmd: 'work',
      name: 'worker',
      readiness: { kind: 'output_contains', needle: 'READY' },
      readyTimeoutMs: 2500,
    });
    expect(run).toHaveBeenCalledWith(
      sandbox.sandboxId,
      ['work'],
      expect.objectContaining({
        detached: true,
        deadline: 'lease',
        process: { name: 'worker', readiness: { kind: 'output_contains', needle: 'READY' } },
      }),
    );
    expect(wait).toHaveBeenCalledExactlyOnceWith(sandbox.sandboxId, 'ready-exec', 3);
    expect(process.command.cmdId).toBe('ready-exec');
  });

  it('reports early exit without killing a named execution whose launch ownership is unknown', async () => {
    const { client, sandbox } = await fixture();
    vi.spyOn(client, 'runCommand').mockResolvedValue({ outcome: 'started', execution_id: 'failed-exec' });
    vi.spyOn(client, 'waitReady').mockResolvedValue({ outcome: 'failed', exit_code: 23 });
    const kill = vi.spyOn(client, 'killCommand');
    const failure = await sandbox
      .start({ cmd: 'work', name: 'worker', readiness: { kind: 'tcp_port', port: 3000 } })
      .catch((error: unknown) => error);
    expect(failure).toBeInstanceOf(ServerExitedError);
    expect(failure).toMatchObject({ exitCode: 23, executionId: 'failed-exec' });
    expect(kill).not.toHaveBeenCalled();
  });

  it.each<ListProcessesResponse>([
    { outcome: 'unsupported' },
    {
      outcome: 'processes',
      processes: [
        { execution_id: 'x', name: 'worker', state: 'running' },
        { execution_id: 'y', name: 'worker', state: 'running' },
      ],
    },
  ])('refuses unavailable or ambiguous discovery instead of fabricating process data', async (inventory) => {
    const { client, sandbox } = await fixture();
    client.listProcesses = async () => inventory;
    await expect(sandbox.processes()).rejects.toMatchObject({
      code: inventory.outcome === 'unsupported' ? 'unsupported' : 'invalid_response',
    });
  });

  it('validates a process name before sending any launch', async () => {
    const { client, sandbox } = await fixture();
    const run = vi.spyOn(client, 'runCommand');
    await expect(sandbox.start({ cmd: 'work', name: 'invalid name' })).rejects.toMatchObject({
      code: 'invalid_request',
    });
    expect(run).not.toHaveBeenCalled();
  });
});
