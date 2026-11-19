import { afterEach, describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';
import type { RunCommandResponse, WaitSandboxReadyResponse } from '../src/index.ts';

const cleanup: Array<() => Promise<void>> = [];

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

afterEach(async () => {
  for (const stop of cleanup.splice(0)) await stop();
});

async function fixture() {
  const client = new MemorySandboxClient({
    commands: {
      serve: ({ signal }) =>
        new Promise((resolve) => {
          signal.addEventListener('abort', () => resolve({ exitCode: 143 }), { once: true });
        }),
    },
  });
  const sandbox = await Sandbox.create(client, { tenant: tenantId('named-origin') });
  cleanup.push(async () => {
    const state = await client.listProcesses(sandbox.sandboxId);
    if (state.outcome === 'processes') {
      for (const command of state.processes) await client.killCommand(sandbox.sandboxId, command.execution_id, 'kill');
    }
  });
  return { client, sandbox };
}

function started(response: RunCommandResponse): string {
  expect(response.outcome).toBe('started');
  if (response.outcome !== 'started') throw new Error('detached command must start');
  return response.execution_id;
}

function reportCreated(client: MemorySandboxClient) {
  const run = client.runCommand.bind(client);
  client.runCommand = async (...args: Parameters<MemorySandboxClient['runCommand']>) => {
    const response = await run(...args);
    return response.outcome === 'started' ? { ...response, origin: 'created' as const } : response;
  };
}

describe('atomic named command origin', () => {
  it('distinguishes the creating launch from a concurrent reused name and preserves replay provenance', async () => {
    const { client, sandbox } = await fixture();
    const options = { detached: true, process: { name: 'web' }, deadline: 'lease' as const };
    const [created, reused] = await Promise.all([
      client.runCommand(sandbox.sandboxId, ['serve'], { ...options, launchKey: 'creator' }),
      client.runCommand(sandbox.sandboxId, ['serve'], { ...options, launchKey: 'joiner' }),
    ]);
    const id = started(created);
    expect(started(reused)).toBe(id);
    const replay = await client.runCommand(sandbox.sandboxId, ['serve'], { ...options, launchKey: 'creator' });
    expect(await client.pollCommand(sandbox.sandboxId, id)).toEqual({ outcome: 'running' });
    const inventory = await client.listProcesses(sandbox.sandboxId);
    expect(inventory.outcome === 'processes' ? inventory.processes.length : 0).toBe(1);
    expect(created).toMatchObject({ origin: 'created' });
    expect(reused).toMatchObject({ origin: 'reused' });
    expect(replay).toEqual(created);
  });

  it('stops a positively created named server after readiness fails without affecting a sibling', async () => {
    const { client, sandbox } = await fixture();
    const sibling = await client.runCommand(sandbox.sandboxId, ['serve'], {
      detached: true,
      process: { name: 'sibling' },
    });
    reportCreated(client);
    const launched: string[] = [];
    const run = client.runCommand.bind(client);
    client.runCommand = async (...args: Parameters<MemorySandboxClient['runCommand']>) => {
      const response = await run(...args);
      launched.push(started(response));
      return response;
    };
    client.waitSandboxReady = async () => ({ outcome: 'unsupported' });
    const failure = await sandbox
      .startServer({ cmd: 'serve', name: 'web', port: 3000, preview: false })
      .catch((error: unknown) => error);
    expect(failure).toMatchObject({ code: 'unsupported', executionId: launched[0] });
    expect(await client.pollCommand(sandbox.sandboxId, started(sibling))).toEqual({ outcome: 'running' });
    expect(await client.pollCommand(sandbox.sandboxId, launched[0]!)).toMatchObject({ outcome: 'completed' });
  });

  it('cleans only its positively created named server when canceled during readiness', async () => {
    const { client, sandbox } = await fixture();
    reportCreated(client);
    const entered = deferred<void>();
    const readiness = deferred<WaitSandboxReadyResponse>();
    const launched: string[] = [];
    const run = client.runCommand.bind(client);
    client.runCommand = async (...args: Parameters<MemorySandboxClient['runCommand']>) => {
      const response = await run(...args);
      launched.push(started(response));
      return response;
    };
    client.waitSandboxReady = async () => {
      entered.resolve();
      return readiness.promise;
    };
    const controller = new AbortController();
    const reason = new Error('cancel owned named server');
    const waiting = sandbox
      .startServer({ cmd: 'serve', name: 'web', port: 3000, preview: false, signal: controller.signal })
      .catch((error: unknown) => error);
    await entered.promise;
    controller.abort(reason);
    readiness.resolve({ outcome: 'ready' });
    expect(await waiting).toBe(reason);
    expect(await client.pollCommand(sandbox.sandboxId, launched[0]!)).toMatchObject({ outcome: 'completed' });
  });
});
