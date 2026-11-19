import { afterEach, describe, expect, it, vi } from 'vitest';

import { MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';

const tenant = tenantId('acme');
const sandboxes: Sandbox[] = [];
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
  for (const sandbox of sandboxes.splice(0)) await sandbox.delete();
  for (const end of endings.splice(0)) end();
});

async function fixture(created: boolean | undefined, preserveFirst: boolean) {
  let firstRunning = 0;
  let maxFirstRunning = 0;
  const client = new MemorySandboxClient({
    commands: Object.fromEntries(
      ['first', 'second', 'unrelated'].map((name) => [
        name,
        ({ signal }: { signal: AbortSignal }) => {
          if (name === 'first') {
            firstRunning += 1;
            maxFirstRunning = Math.max(maxFirstRunning, firstRunning);
          }
          return new Promise((resolve) => {
            let running = true;
            const end = () => {
              if (running && name === 'first') firstRunning -= 1;
              running = false;
              resolve({ exitCode: 143 });
            };
            endings.push(end);
            signal.addEventListener('abort', end, { once: true });
          });
        },
      ]),
    ),
  });
  client.waitSandboxReady = async () => ({ outcome: 'ready' });
  const source = await Sandbox.create(client, { tenant, idempotencyKey: 'source' });
  sandboxes.push(source);
  const first = await source.startServer({ cmd: 'first', port: 3000, preview: false });
  const second = await source.startServer({ cmd: 'second', port: 3001, preview: false });
  const receipt = JSON.parse(JSON.stringify(await source.snapshot()));
  if (!preserveFirst) await first.command.kill('term');
  await second.command.kill('term');
  const unrelated = await source.runCommand({ cmd: 'unrelated', detached: true });
  const leased = client.lease.bind(client);
  client.lease = async (request) => ({ ...(await leased(request)), created });
  const primary: Error[] = [];
  client.waitSandboxReady = async (_lease, ports) => {
    if (ports?.includes(3001)) {
      const error = new Error('second server readiness failed');
      primary.push(error);
      throw error;
    }
    return { outcome: 'ready' };
  };
  const firstStarts: string[] = [];
  const run = client.runCommand.bind(client);
  client.runCommand = async (id, command, options) => {
    const response = await run(id, command, options);
    if (command[0] === 'first' && response.outcome === 'started') firstStarts.push(response.execution_id);
    return response;
  };
  const deleted = vi.spyOn(client, 'deleteLease');
  const stopped = vi.spyOn(client, 'stopLease');
  return {
    client,
    source,
    first,
    unrelated,
    firstStarts,
    primary,
    deleted,
    stopped,
    maxFirstRunning: () => maxFirstRunning,
    restore: () =>
      Sandbox.fromSnapshot(client, receipt, { tenant, idempotencyKey: 'source' }).catch((error: unknown) => error),
  };
}

describe('multi-server restore transaction ownership', () => {
  it('waits for confirmed termination after a stop acknowledgement before returning the primary failure', async () => {
    const state = await fixture(false, false);
    const entered = deferred<void>();
    const terminated = deferred<void>();
    let stopping = false;
    const stop = state.client.stopCommand.bind(state.client);
    state.client.stopCommand = async (id, execution, grace) => {
      if (state.firstStarts.includes(execution)) {
        stopping = true;
        return { outcome: 'stopped', signal: 'term' };
      }
      return stop(id, execution, grace);
    };
    const poll = state.client.pollCommand.bind(state.client);
    state.client.pollCommand = async (id, execution) => {
      if (stopping && state.firstStarts.includes(execution)) {
        entered.resolve();
        await terminated.promise;
      }
      return poll(id, execution);
    };
    const pending = state.restore();
    const observed = await Promise.race([entered.promise.then(() => 'confirming'), pending.then(() => 'returned')]);
    if (observed === 'returned') {
      expect(observed).toBe('confirming');
      return;
    }
    expect(await Promise.race([pending.then(() => 'returned'), tick().then(() => 'pending')])).toBe('pending');
    await state.client.killCommand(state.source.sandboxId, state.firstStarts[0]!, 'term');
    terminated.resolve();
    expect(await pending).toBe(state.primary[0]);
  });

  it('preserves the primary failure and reports incomplete cleanup when an owned execution cannot be stopped', async () => {
    const state = await fixture(false, false);
    const stop = state.client.stopCommand.bind(state.client);
    state.client.stopCommand = async (id, execution, grace) => {
      if (state.firstStarts.includes(execution)) throw new Error('stop unavailable');
      return stop(id, execution, grace);
    };
    const kill = state.client.killCommand.bind(state.client);
    state.client.killCommand = async (id, execution, signal) => {
      if (state.firstStarts.includes(execution)) throw new Error('kill unavailable');
      return kill(id, execution, signal);
    };
    const failure = await state.restore();
    expect(failure).toBe(state.primary[0]);
    expect(failure).toMatchObject({ restoreCleanup: { incompleteExecutionIds: [state.firstStarts[0]] } });
    expect(await state.client.pollCommand(state.source.sandboxId, state.firstStarts[0]!)).toEqual({
      outcome: 'running',
    });
    expect(state.deleted).not.toHaveBeenCalled();
  });

  it('reports incomplete cleanup when tracking disappears after stop acknowledgement without termination evidence', async () => {
    const state = await fixture(false, false);
    let stopping = false;
    const stop = state.client.stopCommand.bind(state.client);
    state.client.stopCommand = async (id, execution, grace) => {
      if (state.firstStarts.includes(execution)) {
        stopping = true;
        return { outcome: 'stopped', signal: 'term' };
      }
      return stop(id, execution, grace);
    };
    const poll = state.client.pollCommand.bind(state.client);
    state.client.pollCommand = async (id, execution) =>
      stopping && state.firstStarts.includes(execution) ? { outcome: 'not_found' } : poll(id, execution);
    const failure = await state.restore();
    expect(failure).toBe(state.primary[0]);
    expect(failure).toMatchObject({ restoreCleanup: { incompleteExecutionIds: [state.firstStarts[0]] } });
    expect(await poll(state.source.sandboxId, state.firstStarts[0]!)).toEqual({ outcome: 'running' });
    expect(state.deleted).not.toHaveBeenCalled();
  });

  it.each([false, undefined])(
    'rolls back its newly launched unnamed servers before retrying a failed restore on a reused/unknown grant (%s)',
    async (created) => {
      const state = await fixture(created, false);
      const firstFailure = await state.restore();
      const repeatedFailure = await state.restore();
      expect(firstFailure).toBe(state.primary[0]);
      expect(repeatedFailure).toBe(state.primary[1]);
      expect(state.maxFirstRunning()).toBe(1);
      expect(state.firstStarts).toHaveLength(2);
      for (const execution of state.firstStarts) {
        expect(await state.client.pollCommand(state.source.sandboxId, execution)).toMatchObject({
          outcome: 'completed',
          exit_code: 143,
        });
      }
      expect(await state.client.pollCommand(state.source.sandboxId, state.unrelated.cmdId)).toEqual({
        outcome: 'running',
      });
      expect(state.deleted).not.toHaveBeenCalled();
      expect(state.stopped).not.toHaveBeenCalled();
    },
  );

  it.each([false, undefined])(
    'preserves an adopted first execution when another server fails on a reused/unknown grant (%s)',
    async (created) => {
      const state = await fixture(created, true);
      const failure = await state.restore();
      expect(failure).toBe(state.primary[0]);
      expect(state.firstStarts).toHaveLength(0);
      expect(await state.client.pollCommand(state.source.sandboxId, state.first.command.cmdId)).toEqual({
        outcome: 'running',
      });
      expect(await state.client.pollCommand(state.source.sandboxId, state.unrelated.cmdId)).toEqual({
        outcome: 'running',
      });
      expect(state.deleted).not.toHaveBeenCalled();
      expect(state.stopped).not.toHaveBeenCalled();
    },
  );
});
