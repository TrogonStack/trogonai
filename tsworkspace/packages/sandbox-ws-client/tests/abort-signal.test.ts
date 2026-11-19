import { describe, expect, it } from 'vitest';

import { Command, CommandCancellationError, CommandFinished, Sandbox, SandboxCapabilities } from '../src/index.ts';
import type { LeaseId, PollCommandResponse, RunCommandResponse, SandboxClientApi } from '../src/index.ts';

const leaseId = 'lease-1' as LeaseId;
const completed: PollCommandResponse = { outcome: 'completed', exit_code: 137, stdout: [], stderr: [] };
const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function fixture(features: string[] | null = ['command_cancellation']) {
  const start = deferred<RunCommandResponse>();
  const states = new Map<string, PollCommandResponse>([
    ['owned', { outcome: 'running' }],
    ['sibling', { outcome: 'running' }],
  ]);
  const sent: Array<Parameters<SandboxClientApi['runCommand']>> = [];
  const kills: Array<{ id: string; signal: string }> = [];
  const controls = { completeOnKill: true, killFailure: false, pollFailure: false };
  const client = {
    runCommand: (...args: Parameters<SandboxClientApi['runCommand']>) => {
      sent.push(args);
      return start.promise;
    },
    killCommand: async (_lease: LeaseId, id: string, signal = 'term') => {
      kills.push({ id, signal });
      if (controls.killFailure) throw new Error('connection lost');
      if (controls.completeOnKill) states.set(id, completed);
      return { outcome: 'signaled' as const };
    },
    pollCommand: async (_lease: LeaseId, id: string) => {
      if (controls.pollFailure) throw new Error('poll connection lost');
      return states.get(id) ?? { outcome: 'not_found' as const };
    },
  } as unknown as SandboxClientApi;
  const sandbox = new Sandbox(client, {
    sandboxId: leaseId,
    provider: 'e2b',
    deadline: new Date(Date.now() + 60_000),
    ...(features === null ? {} : { capabilities: SandboxCapabilities.from(features) }),
  });
  return { sandbox, client, start, states, sent, kills, controls };
}

describe('verified command cancellation', () => {
  it('does not launch a pre-aborted command', async () => {
    const f = fixture();
    const controller = new AbortController();
    const reason = new Error('stop before launch');
    controller.abort(reason);
    await expect(f.sandbox.runCommand({ cmd: 'sleep', signal: controller.signal })).rejects.toBe(reason);
    expect(f.sent).toEqual([]);
  });

  it('obtains an early execution identity for a signal-bound foreground command', async () => {
    const f = fixture();
    const controller = new AbortController();
    const pending = f.sandbox.runCommand({ cmd: 'true', signal: controller.signal });
    f.states.set('owned', { outcome: 'completed', exit_code: 7, stdout: [111, 107], stderr: [] });
    f.start.resolve({ outcome: 'started', execution_id: 'owned' });
    const result = await pending;
    expect(result).toBeInstanceOf(CommandFinished);
    expect(result.exitCode).toBe(7);
    expect(result.stdout()).toBe('ok');
    expect(f.sent[0]?.[2]?.detached).toBe(true);
    controller.abort();
    await tick();
    expect(f.kills).toEqual([]);
  });

  it('awaits a late owned acknowledgement and confirmed stop without touching a sibling', async () => {
    const f = fixture();
    const controller = new AbortController();
    const reason = new Error('cancel in flight');
    const pending = f.sandbox.runCommand({ cmd: 'sleep', signal: controller.signal }).catch((error: unknown) => error);
    controller.abort(reason);
    const observed = await Promise.race([pending, tick().then(() => 'pending')]);
    f.start.resolve({ outcome: 'started', execution_id: 'owned' });
    expect(observed).toBe('pending');
    const error = await pending;
    expect(error).toBeInstanceOf(CommandCancellationError);
    expect(error).toMatchObject({
      code: 'command_cancelled',
      cleanupPending: false,
      outcomeUnknown: false,
      executionId: 'owned',
      cause: reason,
    });
    expect(f.kills).toEqual([{ id: 'owned', signal: 'kill' }]);
    expect(f.states.get('owned')?.outcome).toBe('completed');
    expect(f.states.get('sibling')?.outcome).toBe('running');
  });

  it('does not treat signal delivery as confirmed completion', async () => {
    const f = fixture();
    f.controls.completeOnKill = false;
    const running = f.sandbox.runCommand({ cmd: 'sleep', detached: true });
    f.start.resolve({ outcome: 'started', execution_id: 'owned' });
    const command = await running;
    const cancelling = command.cancel({ timeoutMs: 500 }).then(
      () => 'done',
      (error: unknown) => error,
    );
    await tick();
    const observed = await Promise.race([cancelling, tick().then(() => 'pending')]);
    f.states.set('owned', completed);
    expect(observed).toBe('pending');
    expect(await cancelling).toBe('done');
    expect(f.kills).toEqual([{ id: 'owned', signal: 'kill' }]);
  });

  it('shares cancellation and lets a returned detached handle observe automatic cleanup', async () => {
    const f = fixture();
    const controller = new AbortController();
    const reason = new Error('cancel detached');
    const running = f.sandbox.runCommand({ cmd: 'sleep', detached: true, signal: controller.signal });
    f.start.resolve({ outcome: 'started', execution_id: 'owned' });
    const command = await running;
    expect(command).toBeInstanceOf(Command);
    controller.abort(reason);
    const first = command.cancel();
    const second = command.cancel();
    void first.catch(() => {});
    void second.catch(() => {});
    const sameCancellation = second === first;
    await Promise.allSettled([first, second]);
    expect(sameCancellation).toBe(true);
    expect((await first).exitCode).toBe(137);
    await expect(command.wait()).rejects.toMatchObject({
      code: 'command_cancelled',
      cleanupPending: false,
      cause: reason,
    });
    expect(f.kills).toEqual([{ id: 'owned', signal: 'kill' }]);
  });

  it.each(['not_found', 'running', 'poll_error', 'kill_error'] as const)(
    'reports uncertain cleanup for %s',
    async (failure) => {
      const f = fixture();
      const controller = new AbortController();
      const reason = new Error('cancel with missing proof');
      const running = f.sandbox.runCommand({
        cmd: 'sleep',
        detached: true,
        signal: controller.signal,
        cancellationTimeoutMs: 30,
      });
      f.start.resolve({ outcome: 'started', execution_id: 'owned' });
      const command = await running;
      f.controls.completeOnKill = false;
      if (failure === 'not_found') f.states.delete('owned');
      if (failure === 'poll_error') f.controls.pollFailure = true;
      if (failure === 'kill_error') f.controls.killFailure = true;
      controller.abort(reason);
      await expect(command.cancel()).rejects.toMatchObject({
        code: 'cancellation_incomplete',
        cleanupPending: true,
        outcomeUnknown: true,
        executionId: 'owned',
        cause: reason,
      });
      await expect(command.wait()).rejects.toMatchObject({ cleanupPending: true, outcomeUnknown: true });
      expect(f.states.get('sibling')?.outcome).toBe('running');
    },
  );

  it('bounds an unknown acknowledgement and retains eventual owned cleanup', async () => {
    const f = fixture();
    const controller = new AbortController();
    const reason = new Error('acknowledgement lost');
    const running = f.sandbox.runCommand({ cmd: 'sleep', signal: controller.signal, cancellationTimeoutMs: 20 });
    controller.abort(reason);
    await expect(running).rejects.toMatchObject({
      code: 'cancellation_incomplete',
      cleanupPending: true,
      outcomeUnknown: true,
      cause: reason,
    });
    f.start.resolve({ outcome: 'started', execution_id: 'owned' });
    await tick();
    expect(f.kills).toEqual([{ id: 'owned', signal: 'kill' }]);
    expect(f.states.get('sibling')?.outcome).toBe('running');
  });

  it('refuses a signal on named work before it can attach to a reused execution', async () => {
    const f = fixture();
    const controller = new AbortController();
    const running = f.sandbox.runCommand({
      cmd: 'serve',
      detached: true,
      process: { name: 'shared' },
      signal: controller.signal,
    });
    f.start.resolve({ outcome: 'started', execution_id: 'sibling' });
    await expect(running).rejects.toMatchObject({ code: 'unsupported' });
    expect(f.sent).toEqual([]);
    expect(f.kills).toEqual([]);
  });

  it.each([null, []])('refuses unknown or absent cancellation capability before launch', async (features) => {
    const f = fixture(features);
    const controller = new AbortController();
    const running = f.sandbox.runCommand({ cmd: 'sleep', signal: controller.signal });
    f.start.resolve({ outcome: 'completed', exit_code: 0, stdout: [], stderr: [] });
    await expect(running).rejects.toMatchObject({ code: 'unsupported' });
    expect(f.sent).toEqual([]);
  });

  it.each([0, -1, 1.5, 60_001, Number.NaN])(
    'validates cancellation budget %s before launch',
    async (cancellationTimeoutMs) => {
      const f = fixture();
      const controller = new AbortController();
      const running = f.sandbox.runCommand({ cmd: 'sleep', signal: controller.signal, cancellationTimeoutMs });
      f.start.resolve({ outcome: 'completed', exit_code: 0, stdout: [], stderr: [] });
      await expect(running).rejects.toMatchObject({ code: 'invalid_request' });
      expect(f.sent).toEqual([]);
    },
  );

  it('leaves a run without a signal unchanged', async () => {
    const f = fixture([]);
    const pending = f.sandbox.runCommand({ cmd: 'true' });
    f.start.resolve({ outcome: 'completed', exit_code: 0, stdout: [], stderr: [] });
    expect((await pending).exitCode).toBe(0);
    expect(f.sent[0]?.[2]?.detached).toBeUndefined();
    expect(f.kills).toEqual([]);
  });

  it('replays confirmed completion when the signal aborts after a detached wait finished', async () => {
    const f = fixture();
    const controller = new AbortController();
    const running = f.sandbox.runCommand({ cmd: 'true', detached: true, signal: controller.signal });
    f.start.resolve({ outcome: 'started', execution_id: 'owned' });
    const command = await running;
    f.states.set('owned', { outcome: 'completed', exit_code: 0, stdout: [], stderr: [] });
    expect((await command.wait()).exitCode).toBe(0);
    controller.abort();
    const repeated = await Promise.race([command.wait(), tick().then(() => 'pending')]);
    expect(repeated).toBeInstanceOf(CommandFinished);
    expect(f.kills).toEqual([]);
  });

  it('returns completed state after explicit cancellation outlives its signal listener', async () => {
    const f = fixture();
    const controller = new AbortController();
    const running = f.sandbox.runCommand({ cmd: 'sleep', detached: true, signal: controller.signal });
    f.start.resolve({ outcome: 'started', execution_id: 'owned' });
    const command = await running;
    expect((await command.cancel()).exitCode).toBe(137);
    controller.abort(new Error('explicit cancellation already finished'));
    const repeated = await Promise.race([command.wait(), tick().then(() => 'pending')]);
    expect(repeated).toBeInstanceOf(CommandFinished);
    expect(f.kills).toEqual([{ id: 'owned', signal: 'kill' }]);
  });
});
