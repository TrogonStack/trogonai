import { describe, expect, it } from 'vitest';

import { Sandbox, SandboxCapabilities } from '../src/index.ts';
import type {
  CommandLaunchOrigin,
  LeaseId,
  PollCommandResponse,
  RunCommandResponse,
  SandboxClientApi,
  UnixSignal,
} from '../src/index.ts';

const lease = 'owned-failure-cleanup' as LeaseId;
const completed = { outcome: 'completed' as const, exit_code: 137, stdout: [], stderr: [] };
const sites = ['start_readiness', 'late_named_start', 'server_readiness'] as const;
type Site = (typeof sites)[number];

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

function fixture(origin: CommandLaunchOrigin = 'created') {
  const primary = new Error('the requested setup failed');
  const entered = deferred<void>();
  const start = deferred<RunCommandResponse>();
  const completion = deferred<PollCommandResponse>();
  const state = new Map<string, PollCommandResponse>([
    ['owned', { outcome: 'running' }],
    ['sentinel', { outcome: 'running' }],
  ]);
  const kills: Array<{ lease: LeaseId; id: string; signal?: UnixSignal }> = [];
  const events: string[] = [];
  const controls = { delayStart: false, refuseKill: false, holdCompletion: false };
  let exactKillRequested = false;
  const started: RunCommandResponse = {
    outcome: 'started',
    execution_id: origin === 'reused' ? 'sentinel' : 'owned',
    origin,
  };
  const client = {
    runCommand: async () => {
      entered.resolve();
      return controls.delayStart ? start.promise : started;
    },
    waitReady: async () => {
      throw primary;
    },
    waitSandboxReady: async () => {
      throw primary;
    },
    killCommand: async (target: LeaseId, id: string, signal?: UnixSignal) => {
      kills.push({ lease: target, id, signal });
      events.push(`kill:${id}:${signal}`);
      if (signal === 'kill') {
        exactKillRequested = true;
        if (controls.refuseKill) throw new Error('native kill was refused');
        if (!controls.holdCompletion) state.set(id, completed);
      }
      return { outcome: 'signaled' };
    },
    pollCommand: async (_target: LeaseId, id: string) => {
      if (exactKillRequested && controls.holdCompletion && id === 'owned') {
        events.push(`poll:${id}:pending`);
        return completion.promise;
      }
      const response = state.get(id) ?? { outcome: 'not_found' };
      events.push(`poll:${id}:${response.outcome}`);
      return response;
    },
  } as unknown as SandboxClientApi;
  const sandbox = new Sandbox(client, {
    sandboxId: lease,
    provider: 'local_docker',
    deadline: new Date(Date.now() + 60_000),
    capabilities: SandboxCapabilities.from(['named_processes', 'process_readiness']),
  });
  const begin = (site: Site, signal?: AbortSignal) =>
    site === 'server_readiness'
      ? sandbox.startServer({ cmd: 'serve', name: 'web', port: 3000, preview: false, signal })
      : sandbox.start({ cmd: 'serve', name: 'web', readiness: { kind: 'tcp_port', port: 3000 }, signal });
  const resolveCompleted = () => {
    state.set('owned', completed);
    completion.resolve(completed);
  };
  return { primary, entered, start, started, state, kills, events, controls, sandbox, begin, resolveCompleted };
}

async function failAt(site: Site, f: ReturnType<typeof fixture>): Promise<unknown> {
  if (site !== 'late_named_start') return f.begin(site).catch((error: unknown) => error);
  f.controls.delayStart = true;
  const controller = new AbortController();
  const waiting = f.begin(site, controller.signal).catch((error: unknown) => error);
  await f.entered.promise;
  controller.abort(f.primary);
  const failure = await waiting;
  expect(failure).toBe(f.primary);
  f.start.resolve(f.started);
  await expect.poll(() => f.kills.length).toBe(1);
  return failure;
}

describe('owned command cleanup after setup failure', () => {
  it.each(sites)(
    'uses exact KILL and confirms completion after %s without stronger capability claims',
    async (site) => {
      const f = fixture();
      expect(f.sandbox.supports('command_cancellation')).toBe(false);
      const failure = await failAt(site, f);
      expect(failure).toBe(f.primary);
      expect(f.kills).toEqual([{ lease, id: 'owned', signal: 'kill' }]);
      await expect.poll(() => f.events).toContain('poll:owned:completed');
      expect(f.state.get('owned')).toEqual(completed);
      expect(f.state.get('sentinel')).toEqual({ outcome: 'running' });
      expect(failure).toMatchObject({
        executionId: 'owned',
        commandCleanup: { executionId: 'owned', cleanupPending: false },
      });
    },
  );

  it.each(sites)('keeps the primary error and pending receipt when exact cleanup fails after %s', async (site) => {
    const f = fixture();
    f.controls.refuseKill = true;
    const failure = await failAt(site, f);
    expect(failure).toBe(f.primary);
    expect(f.kills).toEqual([{ lease, id: 'owned', signal: 'kill' }]);
    await expect
      .poll(() => failure)
      .toMatchObject({ executionId: 'owned', commandCleanup: { executionId: 'owned', cleanupPending: true } });
    expect(f.state.get('owned')).toEqual({ outcome: 'running' });
    expect(f.state.get('sentinel')).toEqual({ outcome: 'running' });
  });

  it.each(['start_readiness', 'server_readiness'] as const)(
    'does not mistake a KILL acknowledgement for completion after %s',
    async (site) => {
      const f = fixture();
      f.controls.holdCompletion = true;
      let settled = false;
      const waiting = f
        .begin(site)
        .catch((error: unknown) => error)
        .finally(() => {
          settled = true;
        });
      try {
        await expect.poll(() => f.kills).toEqual([{ lease, id: 'owned', signal: 'kill' }]);
        await expect.poll(() => f.events).toContain('poll:owned:pending');
        expect(settled).toBe(false);
      } finally {
        f.resolveCompleted();
      }
      expect(await waiting).toBe(f.primary);
      expect(f.primary).toMatchObject({ commandCleanup: { executionId: 'owned', cleanupPending: false } });
      expect(f.state.get('sentinel')).toEqual({ outcome: 'running' });
    },
  );

  it('retains an initially unresolved late launch, then settles its exact cleanup receipt', async () => {
    const f = fixture();
    f.controls.delayStart = true;
    f.controls.holdCompletion = true;
    const controller = new AbortController();
    const waiting = f.begin('late_named_start', controller.signal).catch((error: unknown) => error);
    await f.entered.promise;
    controller.abort(f.primary);
    const failure = await waiting;
    expect(failure).toBe(f.primary);
    expect(failure).toMatchObject({ commandCleanup: { cleanupPending: true } });
    f.start.resolve(f.started);
    try {
      await expect.poll(() => f.kills).toEqual([{ lease, id: 'owned', signal: 'kill' }]);
      await expect.poll(() => f.events).toContain('poll:owned:pending');
      expect(failure).toMatchObject({
        executionId: 'owned',
        commandCleanup: { executionId: 'owned', cleanupPending: true },
      });
    } finally {
      f.resolveCompleted();
    }
    await expect.poll(() => failure).toMatchObject({ commandCleanup: { executionId: 'owned', cleanupPending: false } });
    expect(f.state.get('sentinel')).toEqual({ outcome: 'running' });
  });

  it.each(sites.flatMap((site) => (['reused', 'unknown'] as const).map((origin) => ({ site, origin }))))(
    'preserves $origin named work after $site',
    async ({ site, origin }) => {
      const f = fixture(origin);
      let failure: unknown;
      if (site === 'late_named_start') {
        f.controls.delayStart = true;
        const controller = new AbortController();
        const waiting = f.begin(site, controller.signal).catch((error: unknown) => error);
        await f.entered.promise;
        controller.abort(f.primary);
        failure = await waiting;
        f.start.resolve(f.started);
        await expect.poll(() => failure).toMatchObject({ executionId: origin === 'reused' ? 'sentinel' : 'owned' });
      } else {
        failure = await f.begin(site).catch((error: unknown) => error);
      }
      expect(failure).toBe(f.primary);
      expect(f.kills).toEqual([]);
      expect(f.state.get('owned')).toEqual({ outcome: 'running' });
      expect(f.state.get('sentinel')).toEqual({ outcome: 'running' });
    },
  );
});
