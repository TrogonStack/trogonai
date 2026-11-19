import { afterEach, describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';
import type { Command, RunCommandResponse, WaitSandboxReadyResponse } from '../src/index.ts';

const commands: Command[] = [];
const endings: Array<() => void> = [];
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

afterEach(async () => {
  for (const command of commands.splice(0)) await command.kill('term');
  for (const end of endings.splice(0)) end();
});

async function fixture() {
  const client = new MemorySandboxClient({
    commands: {
      serve: ({ signal }) =>
        new Promise((resolve) => {
          const end = () => resolve({ exitCode: 143 });
          endings.push(end);
          signal.addEventListener('abort', end, { once: true });
        }),
    },
  });
  const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
  const existing = await sandbox.runCommand({ cmd: 'serve', detached: true, process: { name: 'web' } });
  commands.push(existing);
  return { client, sandbox, existing };
}

describe('named server cancellation ownership', () => {
  it('cancels locally while readiness is pending without killing a potentially reused execution', async () => {
    const { client, sandbox, existing } = await fixture();
    client.runCommand = async () => ({ outcome: 'started', execution_id: existing.cmdId });
    const entered = deferred<void>();
    const ready = deferred<WaitSandboxReadyResponse>();
    client.waitSandboxReady = async () => {
      entered.resolve();
      return ready.promise;
    };
    const controller = new AbortController();
    const reason = new Error('cancel waiting for named server');
    const pending = sandbox
      .startServer({ cmd: 'serve', name: 'web', port: 3000, preview: false, signal: controller.signal })
      .then(
        () => 'started',
        (error: unknown) => error,
      );
    await entered.promise;
    controller.abort(reason);
    const observed = await Promise.race([pending, tick().then(() => 'pending')]);
    ready.resolve({ outcome: 'ready' });
    await pending;
    expect(await client.pollCommand(sandbox.sandboxId, existing.cmdId)).toEqual({ outcome: 'running' });
    expect(observed).toBe(reason);
  });

  it('does not kill an existing named execution whose acknowledgement arrives after cancellation', async () => {
    const { client, sandbox, existing } = await fixture();
    const acknowledgement = deferred<RunCommandResponse>();
    client.runCommand = async () => acknowledgement.promise;
    const controller = new AbortController();
    const reason = new Error('cancel named launch');
    const pending = sandbox
      .startServer({ cmd: 'serve', name: 'web', port: 3000, preview: false, signal: controller.signal })
      .then(
        () => 'started',
        (error: unknown) => error,
      );
    controller.abort(reason);
    const observed = await Promise.race([pending, tick().then(() => 'pending')]);
    acknowledgement.resolve({ outcome: 'started', execution_id: existing.cmdId });
    await tick();
    expect(await pending).toBe(reason);
    expect(observed).toBe(reason);
    expect(await client.pollCommand(sandbox.sandboxId, existing.cmdId)).toEqual({ outcome: 'running' });
  });
});
