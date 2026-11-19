import { afterEach, describe, expect, it } from 'vitest';

import { Command, Sandbox, SandboxCapabilities, SandboxWsError, TransportError } from '../src/index.ts';
import type { LeaseId, PollCommandResponse, RunCommandResponse, SandboxClientApi } from '../src/index.ts';

const lease = 'launch-cancellation-lease' as LeaseId;
const cleanup: Array<() => Promise<unknown>> = [];
const completed = { outcome: 'completed' as const, exit_code: 137, stdout: [], stderr: [] };
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

afterEach(async () => {
  for (const stop of cleanup.splice(0)) await stop();
});

function fixture() {
  const controller = new AbortController();
  const start = deferred<RunCommandResponse>();
  const sent: Array<Parameters<SandboxClientApi['runCommand']>> = [];
  const cancellations: Array<{ lease: LeaseId; key: string }> = [];
  const kills: string[] = [];
  const state = new Map<string, PollCommandResponse>([
    ['owned', { outcome: 'running' }],
    ['sibling', { outcome: 'running' }],
  ]);
  const controls = { pending: false, preserved: false, loseFirstCancellationReply: false, retryAfterMs: 0 };
  const client = {
    runCommand: (...args: Parameters<SandboxClientApi['runCommand']>) => {
      sent.push(args);
      return start.promise;
    },
    cancelCommandLaunch: async (target: LeaseId, key: string) => {
      cancellations.push({ lease: target, key });
      if (controls.retryAfterMs > 0)
        throw new SandboxWsError('rate_limited', 'control retry was delayed', {
          retryable: true,
          retryAfterMs: controls.retryAfterMs,
        });
      if (controls.pending) return { outcome: 'pending', message: 'exact launch cleanup is still pending' };
      if (controls.preserved) return { outcome: 'preserved', execution_id: 'sibling' };
      state.set('owned', completed);
      if (controls.loseFirstCancellationReply && cancellations.length === 1)
        throw new Error('cancellation reply was lost');
      return { outcome: 'canceled', execution_id: 'owned' };
    },
    killCommand: async (_target: LeaseId, id: string) => {
      kills.push(id);
      state.set(id, completed);
      return { outcome: 'signaled' };
    },
    pollCommand: async (_target: LeaseId, id: string) => state.get(id) ?? { outcome: 'not_found' },
  } as unknown as SandboxClientApi;
  const sandbox = new Sandbox(client, {
    sandboxId: lease,
    provider: 'e2b',
    deadline: new Date(Date.now() + 60_000),
    capabilities: SandboxCapabilities.from(['command_cancellation', 'command_launch_cancellation']),
  });
  const run = (process?: { name: string }) => {
    const result = sandbox
      .runCommand({ cmd: 'serve', detached: true, signal: controller.signal, cancellationTimeoutMs: 40, process })
      .catch((error: unknown) => error);
    cleanup.push(async () => {
      controller.abort(new Error('fixture cleanup'));
      return result;
    });
    return result;
  };
  return { controller, start, sent, cancellations, kills, state, controls, run, client };
}

describe('command launch cancellation before acknowledgement', () => {
  it('retains a bounded launch reference and requests the stronger protocol before dispatch', async () => {
    const f = fixture();
    void f.run();
    expect(f.sent).toHaveLength(1);
    expect(f.sent[0]?.[2]).toMatchObject({ requireLaunchCancellation: true });
    expect(f.sent[0]?.[2]?.launchKey).toMatch(/^[A-Za-z0-9_-]{1,128}$/);
  });

  it('confirms cancellation with a permanently lost start reply and preserves a sibling', async () => {
    const f = fixture();
    const reason = new Error('cancel while start reply is lost');
    const result = f.run();
    f.controller.abort(reason);
    const error = await result;
    const key = f.sent[0]?.[2]?.launchKey;
    expect(error).toMatchObject({
      code: 'command_cancelled',
      cleanupPending: false,
      outcomeUnknown: false,
      executionId: 'owned',
      launchKey: key,
      cause: reason,
    });
    expect(f.cancellations).toEqual([{ lease, key }]);
    expect(f.state.get('owned')).toEqual(completed);
    expect(f.state.get('sibling')).toEqual({ outcome: 'running' });
    expect(f.kills).toEqual([]);
    f.start.resolve({ outcome: 'started', execution_id: 'owned', origin: 'created' });
    await tick();
    expect(f.kills).toEqual([]);
  });

  it('retries the same launch authority when the cancellation acknowledgement is lost', async () => {
    const f = fixture();
    f.controls.loseFirstCancellationReply = true;
    const result = f.run();
    f.controller.abort(new Error('stop owned launch'));
    expect(await result).toMatchObject({ code: 'command_cancelled', cleanupPending: false });
    expect(f.cancellations).toHaveLength(2);
    expect(f.cancellations[0]).toEqual(f.cancellations[1]);
    expect(f.state.get('sibling')).toEqual({ outcome: 'running' });
  });

  it('retains reconciliation authority when bounded cleanup remains pending', async () => {
    const f = fixture();
    f.controls.pending = true;
    const result = f.run();
    f.controller.abort(new Error('pending owned launch'));
    expect(await result).toMatchObject({
      code: 'cancellation_incomplete',
      cleanupPending: true,
      outcomeUnknown: true,
      launchKey: f.sent[0]?.[2]?.launchKey,
    });
    expect(f.cancellations.length).toBeGreaterThan(0);
    expect(f.state.get('owned')).toEqual({ outcome: 'running' });
    expect(f.kills).toEqual([]);
  });

  it('does not retry before a server delay that exceeds the remaining cancellation budget', async () => {
    const f = fixture();
    f.controls.retryAfterMs = 1_000;
    const result = f.run();
    f.controller.abort(new Error('cancel under a rate limit'));
    expect(await result).toMatchObject({
      code: 'cancellation_incomplete',
      cleanupPending: true,
      launchKey: f.sent[0]?.[2]?.launchKey,
    });
    expect(f.cancellations).toHaveLength(1);
    expect(f.state.get('owned')).toEqual({ outcome: 'running' });
  });

  it('reports preservation of reused named work without claiming that its process stopped', async () => {
    const f = fixture();
    f.controls.preserved = true;
    const result = f.run({ name: 'shared' });
    f.controller.abort(new Error('cancel only this named join'));
    expect(await result).toMatchObject({
      code: 'command_preserved',
      preserved: true,
      cleanupPending: false,
      outcomeUnknown: false,
      executionId: 'sibling',
      launchKey: f.sent[0]?.[2]?.launchKey,
    });
    expect(f.state.get('sibling')).toEqual({ outcome: 'running' });
    expect(f.kills).toEqual([]);
  });

  it.each([false, true])(
    'retains launch authority when the start transport rejects before abort (frozen error: %s)',
    async (frozen) => {
      const f = fixture();
      const transport = new TransportError('the start reply was lost', { outcomeUnknown: true });
      if (frozen) Object.freeze(transport);
      const running = f.run();
      f.start.reject(transport);
      const failure = await running;
      expect(failure).toMatchObject({ code: 'transport', outcomeUnknown: true, launchKey: f.sent[0]?.[2]?.launchKey });
      if (!frozen) expect(failure).toBe(transport);
      const key = (failure as { launchKey?: string }).launchKey;
      expect(key).toMatch(/^[A-Za-z0-9_-]{1,128}$/);
      if (key === undefined) throw new Error('uncertain launch failure must retain its reconciliation reference');
      await expect(f.client.cancelCommandLaunch(lease, key)).resolves.toEqual({
        outcome: 'canceled',
        execution_id: 'owned',
      });
      expect(f.state.get('owned')).toEqual(completed);
      expect(f.state.get('sibling')).toEqual({ outcome: 'running' });
    },
  );

  it.each(['created', 'reused', 'unknown'] as const)(
    'keeps acknowledged named %s ownership fenced when its signal aborts later',
    async (origin) => {
      const f = fixture();
      f.controls.preserved = origin === 'reused';
      f.controls.pending = origin === 'unknown';
      const result = f.run({ name: 'shared' });
      const id = origin === 'created' ? 'owned' : 'sibling';
      f.start.resolve({ outcome: 'started', execution_id: id, origin });
      const command = await result;
      expect(command).toBeInstanceOf(Command);
      if (!(command instanceof Command)) throw new Error('the named launch must return a command handle');
      f.controller.abort(new Error('cancel after acknowledgement'));
      const failure = await command.wait().catch((error: unknown) => error);
      expect(failure).toMatchObject({
        code:
          origin === 'created'
            ? 'command_cancelled'
            : origin === 'reused'
              ? 'command_preserved'
              : 'cancellation_incomplete',
        cleanupPending: origin === 'unknown',
        outcomeUnknown: origin === 'unknown',
        launchKey: f.sent[0]?.[2]?.launchKey,
      });
      expect(f.kills).toEqual(origin === 'created' ? ['owned'] : []);
      expect(f.state.get('sibling')).toEqual({ outcome: 'running' });
    },
  );
});
