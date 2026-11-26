import { describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, ServerExitedError, TimeoutError, tenantId } from '../src/index.ts';
import type { CommandHandler, WaitSandboxReadyResponse } from '../src/index.ts';

const PREVIEW = 'https://gw/v1/preview/l/3000/tok/';

/** A server that runs until it is killed. */
const forever: CommandHandler = ({ signal }) =>
  new Promise((resolve) => {
    signal.addEventListener('abort', () => resolve({ exitCode: 143 }), { once: true });
  });

async function setup(commands: Record<string, CommandHandler>, ready: WaitSandboxReadyResponse[] | 'never' = []) {
  const client = new MemorySandboxClient({ commands });
  const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
  const ports: Array<number[] | undefined> = [];
  client.waitSandboxReady = (async (_lease, p) => {
    ports.push(p);
    return ready === 'never'
      ? { outcome: 'not_ready', pending_ports: p ?? [] }
      : (ready.shift() ?? { outcome: 'ready' });
  }) as typeof client.waitSandboxReady;
  client.getSandboxDomain = async () => ({
    outcome: 'available',
    endpoint: 'h:3000',
    url: PREVIEW,
    expires_at: 1_700_000_000,
  });
  return { client, sandbox, ports };
}

describe('Sandbox.startServer', () => {
  it('starts the process, waits for the port and returns the preview url', async () => {
    const { client, sandbox, ports } = await setup({ serve: forever }, [
      { outcome: 'not_ready', pending_ports: [3000] },
    ]);
    const server = await sandbox.startServer({ cmd: 'serve', port: 3000, name: 'web' });
    expect(server).toMatchObject({ name: 'web', port: 3000, url: PREVIEW, expiresAt: new Date(1_700_000_000_000) });
    expect(ports).toEqual([[3000], [3000]]);
    const state = await client.pollCommand(sandbox.sandboxId, server.command.cmdId);
    expect(state.outcome).toBe('running');
    await server.command.kill('term');
  });

  it('skips the preview lookup when asked', async () => {
    const { sandbox, client } = await setup({ serve: forever });
    client.getSandboxDomain = async () => {
      throw new Error('should not be called');
    };
    const server = await sandbox.startServer({ cmd: 'serve', port: 3000, preview: false });
    expect(server.url).toBeUndefined();
    await server.command.kill('term');
  });

  it('waits for the path to answer from inside the sandbox', async () => {
    const statuses = ['000', '503', '404'];
    const probes: string[][] = [];
    const { sandbox } = await setup({
      serve: forever,
      sh: ({ args }) => {
        probes.push(args);
        return { exitCode: 0, stdout: statuses.shift() ?? '200' };
      },
    });
    const server = await sandbox.startServer({ cmd: 'serve', port: 3000, path: '/healthz' });
    expect(server.url).toBe(PREVIEW);
    expect(probes).toHaveLength(3);
    expect(probes[0]?.at(-1)).toBe('http://127.0.0.1:3000/healthz');
    await server.command.kill('term');
  });

  it('fails with the process output when it exits before the port opens', async () => {
    const { sandbox } = await setup(
      { serve: () => ({ exitCode: 1, stdout: 'starting\n', stderr: 'EADDRINUSE: address already in use\n' }) },
      'never',
    );
    const err = await sandbox.startServer({ cmd: 'serve', port: 3000, timeoutMs: 5000 }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(ServerExitedError);
    expect(err).toMatchObject({ code: 'command_failed', exitCode: 1 });
    expect((err as ServerExitedError).message).toContain('EADDRINUSE');
    expect((err as ServerExitedError).output).toContain('starting');
  });

  it('times out, stops the process and reports the port', async () => {
    let aborted = false;
    const { sandbox } = await setup(
      {
        serve: ({ signal }) =>
          new Promise((resolve) => {
            signal.addEventListener('abort', () => {
              aborted = true;
              resolve({ exitCode: 143 });
            });
          }),
      },
      'never',
    );
    const failure = sandbox.startServer({ cmd: 'serve', port: 3000, timeoutMs: 50 });
    await expect(failure).rejects.toBeInstanceOf(TimeoutError);
    await expect(failure).rejects.toThrow(/port 3000 to accept connections/);
    expect(aborted).toBe(true);
  });

  it('times out when the path never answers', async () => {
    const { sandbox } = await setup({ serve: forever, sh: () => ({ exitCode: 0, stdout: '503' }) });
    await expect(sandbox.startServer({ cmd: 'serve', port: 3000, path: '/up', timeoutMs: 50 })).rejects.toThrow(
      /\/up on port 3000 to answer/,
    );
  });

  it('stops the process when the provider cannot check ports', async () => {
    const { sandbox } = await setup({ serve: forever }, [{ outcome: 'unsupported' }]);
    await expect(sandbox.startServer({ cmd: 'serve', port: 3000 })).rejects.toMatchObject({ code: 'unsupported' });
  });

  it('stops the process when the readiness check fails', async () => {
    let stopped = false;
    const { sandbox, client } = await setup({
      serve: ({ signal }) =>
        new Promise((resolve) => {
          signal.addEventListener('abort', () => {
            stopped = true;
            resolve({ exitCode: 143 });
          });
        }),
    });
    client.waitSandboxReady = async () => {
      throw new Error('connection lost');
    };
    await expect(sandbox.startServer({ cmd: 'serve', port: 3000 })).rejects.toThrow('connection lost');
    expect(stopped).toBe(true);
  });

  it('rejects bad input before starting anything', async () => {
    const { sandbox } = await setup({ serve: forever });
    await expect(sandbox.startServer({ cmd: 'serve', port: 0 })).rejects.toThrow(RangeError);
    await expect(sandbox.startServer({ cmd: 'serve', port: 80, path: 'healthz' })).rejects.toThrow(RangeError);
    await expect(sandbox.startServer({ cmd: 'serve', port: 80, timeoutMs: -1 })).rejects.toThrow(RangeError);
  });

  describe('after the sandbox comes back', () => {
    /** Ends every running server process, as a pause or restore from snapshot does. */
    async function loseProcesses(aborts: Array<() => void>) {
      for (const abort of aborts.splice(0)) abort();
      await new Promise((resolve) => setTimeout(resolve, 0));
    }

    function servable() {
      const aborts: Array<() => void> = [];
      const starts: string[][] = [];
      const serve: CommandHandler = ({ args, signal }) => {
        starts.push(args);
        return new Promise((resolve) => {
          const end = () => resolve({ exitCode: 137 });
          aborts.push(end);
          signal.addEventListener('abort', end, { once: true });
        });
      };
      return { aborts, starts, serve };
    }

    it('resume starts a server whose process is gone and refreshes the url', async () => {
      const { aborts, starts, serve } = servable();
      const { client, sandbox } = await setup({ serve });
      const server = await sandbox.startServer({ cmd: 'serve', args: ['--dev'], port: 3000, name: 'web' });
      const first = server.command.cmdId;
      await sandbox.pause();
      await loseProcesses(aborts);
      client.getSandboxDomain = async () => ({
        outcome: 'available',
        endpoint: 'h:3000',
        url: 'https://gw/new/',
        expires_at: 1_800_000_000,
      });
      await sandbox.resume();
      expect(starts).toEqual([['--dev'], ['--dev']]);
      expect(server.command.cmdId).not.toBe(first);
      expect(server.url).toBe('https://gw/new/');
      expect(server.expiresAt).toEqual(new Date(1_800_000_000_000));
      expect((await client.pollCommand(sandbox.sandboxId, server.command.cmdId)).outcome).toBe('running');
      await server.stop();
    });

    it('resume leaves a server that is still running alone', async () => {
      const { starts, serve } = servable();
      const { sandbox } = await setup({ serve });
      const server = await sandbox.startServer({ cmd: 'serve', port: 3000 });
      const first = server.command.cmdId;
      await sandbox.pause();
      await sandbox.resume();
      expect(starts).toHaveLength(1);
      expect(server.command.cmdId).toBe(first);
      await server.stop();
    });

    it('wake restarts it too', async () => {
      const { aborts, starts, serve } = servable();
      const { sandbox } = await setup({ serve });
      const server = await sandbox.startServer({ cmd: 'serve', port: 3000 });
      await sandbox.suspend();
      await loseProcesses(aborts);
      await sandbox.wake();
      expect(starts).toHaveLength(2);
      await server.stop();
    });

    it('does not restart with restartOnResume false or after stop()', async () => {
      const { aborts, starts, serve } = servable();
      const { sandbox } = await setup({ serve });
      await sandbox.startServer({ cmd: 'serve', port: 3000, restartOnResume: false });
      const stopped = await sandbox.startServer({ cmd: 'serve', port: 3001 });
      await stopped.stop();
      await loseProcesses(aborts);
      await sandbox.pause();
      await sandbox.resume();
      expect(starts).toHaveLength(2);
    });

    it('restart() replaces a running process', async () => {
      const { starts, serve } = servable();
      const { client, sandbox } = await setup({ serve });
      const server = await sandbox.startServer({ cmd: 'serve', port: 3000 });
      const first = server.command.cmdId;
      await server.restart();
      expect(starts).toHaveLength(2);
      expect((await client.pollCommand(sandbox.sandboxId, first)).outcome).toBe('completed');
      expect((await client.pollCommand(sandbox.sandboxId, server.command.cmdId)).outcome).toBe('running');
      await server.stop();
    });

    it('resume still thaws the sandbox and throws when a server will not come back', async () => {
      const { aborts, serve } = servable();
      const { sandbox, client } = await setup({ serve });
      const server = await sandbox.startServer({ cmd: 'serve', port: 3000 });
      await sandbox.pause();
      await loseProcesses(aborts);
      client.runCommand = async () => {
        throw new Error('no capacity');
      };
      await expect(sandbox.resume()).rejects.toThrow('no capacity');
      expect(server.command.cmdId).toBeDefined();
    });
  });
});
