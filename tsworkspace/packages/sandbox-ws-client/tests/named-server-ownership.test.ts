import { afterEach, describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, SandboxWsError, tenantId } from '../src/index.ts';
import type { Command } from '../src/index.ts';

const commands: Command[] = [];
const endings: Array<() => void> = [];
afterEach(async () => {
  for (const command of commands.splice(0)) await command.kill('term');
  for (const end of endings.splice(0)) end();
});

async function reusedProcess() {
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
  client.runCommand = async () => ({ outcome: 'started', execution_id: existing.cmdId });
  client.waitSandboxReady = async () => ({ outcome: 'ready' });
  return { client, sandbox, existing };
}

describe('named server launch ownership', () => {
  it('preserves a potentially reused named execution when readiness is unsupported and exposes its identity', async () => {
    const { client, sandbox, existing } = await reusedProcess();
    client.waitSandboxReady = async () => ({ outcome: 'unsupported' });
    const failure = await sandbox
      .startServer({ cmd: 'serve', name: 'web', port: 3000, preview: false })
      .catch((error: unknown) => error);
    expect(await client.pollCommand(sandbox.sandboxId, existing.cmdId)).toEqual({ outcome: 'running' });
    expect(failure).toMatchObject({ code: 'unsupported', executionId: existing.cmdId });
  });

  it('preserves both the primary preview failure and the potentially reused named execution', async () => {
    const { client, sandbox, existing } = await reusedProcess();
    const primary = new SandboxWsError('provider', 'preview unavailable');
    client.getSandboxDomain = async () => {
      throw primary;
    };
    const failure = await sandbox
      .startServer({ cmd: 'serve', name: 'web', port: 3000 })
      .catch((error: unknown) => error);
    expect(await client.pollCommand(sandbox.sandboxId, existing.cmdId)).toEqual({ outcome: 'running' });
    expect(failure).toBe(primary);
    expect(failure).toMatchObject({ executionId: existing.cmdId });
  });
});
