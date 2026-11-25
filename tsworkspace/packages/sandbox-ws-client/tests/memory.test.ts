import { describe, expect, it } from 'vitest';

import {
  MEMORY_CAPABILITIES,
  MemorySandboxClient,
  PreviewNotReadyError,
  Sandbox,
  SandboxWsError,
  tenantId,
} from '../src/index.ts';
import type { LeaseId } from '../src/index.ts';

async function create(client = new MemorySandboxClient()) {
  const sandbox = await Sandbox.create(client, { tenant: tenantId('acme'), timeoutMs: 60_000 });
  return { client, sandbox };
}

describe('MemorySandboxClient', () => {
  it('deletes a named sandbox by name and reports not_found afterwards', async () => {
    const client = new MemorySandboxClient();
    await Sandbox.create(client, { tenant: tenantId('acme'), timeoutMs: 60_000, name: 'repl-1' });
    expect(await Sandbox.deleteByName(client, 'repl-1')).toBe(true);
    expect(await Sandbox.deleteByName(client, 'repl-1')).toBe(false);
  });

  it('pauses and resumes a lease idempotently, restoring its remaining time', async () => {
    const { client, sandbox } = await create();
    const id = sandbox.sandboxId as LeaseId;
    const paused = await client.pauseLease(id, 120);
    expect(paused.outcome).toBe('paused');
    expect(await client.pauseLease(id, 5)).toEqual(paused);
    const resumed = await client.resumeLease(id);
    expect(resumed.outcome).toBe('resumed');
    expect(await client.resumeLease(id)).toEqual(resumed);
  });

  it('counts commands and file bytes, reports no gauges, and keeps final figures after stop', async () => {
    const { sandbox } = await create();
    await sandbox.writeFiles([{ path: '/work/a.txt', content: 'hello' }]);
    await sandbox.readFile({ path: '/work/a.txt' });
    await sandbox.runCommand('echo', ['hi']);
    const live = await sandbox.usage();
    expect(live.status).toBe('running');
    expect(live.commandsRun).toBe(1);
    expect(live.fileBytesIn).toBe(5);
    expect(live.fileBytesOut).toBe(5);
    expect(live.commandOutputBytes).toBeGreaterThan(0);
    expect(live.gauges).toBeUndefined();
    expect(live.endedAt).toBeUndefined();
    await sandbox.stop();
    const final = await sandbox.usage();
    expect(final.status).toBe('released');
    expect(final.endedAt).toBeInstanceOf(Date);
    expect(final.commandsRun).toBe(1);
  });

  it('rejects usage for an unknown lease with not_found', async () => {
    const client = new MemorySandboxClient();
    await expect(client.leaseUsage('nope' as LeaseId)).rejects.toMatchObject({ code: 'not_found' });
  });

  it('sums tenant usage over live and released leases', async () => {
    const client = new MemorySandboxClient();
    const a = await create(client);
    const b = await create(client);
    await a.sandbox.writeFiles([{ path: '/x', content: 'abc' }]);
    await b.sandbox.writeFiles([{ path: '/x', content: 'de' }]);
    expect((await client.tenantUsage()).file_bytes_in).toBe(5);
    await b.sandbox.stop();
    const total = await client.tenantUsage();
    expect(total.live_leases).toBe(1);
    expect(total.ended_leases).toBe(1);
    expect(total.file_bytes_in).toBe(5);
  });

  it("forgets a released lease's figures a week after it ended, as the broker does", async () => {
    let nowMs = Date.UTC(2026, 0, 1);
    const client = new MemorySandboxClient({ now: () => nowMs });
    const kept = await create(client);
    const gone = await create(client);
    await kept.sandbox.writeFiles([{ path: '/x', content: 'abc' }]);
    await gone.sandbox.writeFiles([{ path: '/x', content: 'de' }]);
    await gone.sandbox.stop();
    const week = 7 * 24 * 60 * 60 * 1000;
    nowMs += week - 1;
    expect((await client.leaseUsage(gone.sandbox.sandboxId as LeaseId)).status).toBe('released');
    expect((await client.tenantUsage()).ended_leases).toBe(1);
    nowMs += 1;
    await expect(client.leaseUsage(gone.sandbox.sandboxId as LeaseId)).rejects.toMatchObject({ code: 'not_found' });
    const total = await client.tenantUsage();
    expect(total.ended_leases).toBe(0);
    expect(total.live_leases).toBe(1);
    expect(total.file_bytes_in).toBe(3);
  });

  it('reports limits with live lease consumption', async () => {
    const client = new MemorySandboxClient();
    expect((await client.limits()).active_leases).toBe(0);
    const a = await create(client);
    await create(client);
    await a.sandbox.stop();
    const limits = await client.limits();
    expect(limits.active_leases).toBe(1);
    expect(limits.cpu_seconds_cap).toBeGreaterThan(0);
  });

  it('declares what it models and what it does not', () => {
    expect(MEMORY_CAPABILITIES.supported).toContain('read_file');
    expect(MEMORY_CAPABILITIES.unsupported).toContain('fork');
  });

  it('writes then reads files through Sandbox', async () => {
    const { sandbox, client } = await create();
    await sandbox.writeFiles([{ path: '/work/a.txt', content: 'hello' }]);
    expect(await sandbox.readFile({ path: '/work/a.txt' })).toBe('hello');
    expect(new TextDecoder().decode(client.filesystem(sandbox.sandboxId).read('/work/a.txt'))).toBe('hello');
  });

  it('returns null for a missing file', async () => {
    const { sandbox } = await create();
    expect(await sandbox.readFile({ path: '/nope' })).toBeNull();
  });

  it('exits 127 for an unknown command', async () => {
    const { sandbox } = await create();
    const result = await sandbox.runCommand('definitely-not-a-command');
    expect(result.exitCode).toBe(127);
    expect(result.stderr()).toContain('command not found');
  });

  it('runs registered handlers with args, env and the file map', async () => {
    const client = new MemorySandboxClient({
      commands: {
        wc: ({ args, fs }) => ({ exitCode: 0, stdout: `${fs.read(args[0]).length}\n` }),
        greet: ({ env }) => ({ exitCode: 0, stdout: `hi ${env.WHO}` }),
      },
    });
    const { sandbox } = await create(client);
    await sandbox.writeFiles([{ path: 'f', content: 'abcd' }]);
    expect((await sandbox.runCommand('wc', ['f'])).stdout()).toBe('4\n');
    const greeted = await sandbox.runCommand({ cmd: 'greet', env: { WHO: 'bob' } });
    expect(greeted.stdout()).toBe('hi bob');
  });

  it('supports the fs sugar built on built-in commands', async () => {
    const { sandbox } = await create();
    await sandbox.mkDir('/a/b');
    await sandbox.writeFiles([{ path: '/a/b/c.txt', content: 'x' }]);
    expect(await sandbox.fs.exists('/a/b/c.txt')).toBe(true);
    expect(await sandbox.fs.ls('/a/b')).toEqual(['c.txt']);
    await sandbox.fs.rm('/a', { recursive: true });
    expect(await sandbox.fs.exists('/a/b/c.txt')).toBe(false);
  });

  it('polls a detached command to completion', async () => {
    const { sandbox } = await create();
    const cmd = await sandbox.runCommand({ cmd: 'echo', args: ['hi'], detached: true });
    const finished = await cmd.wait({ pollIntervalMs: 1 });
    expect(finished.exitCode).toBe(0);
    expect(finished.stdout()).toBe('hi\n');
  });

  it('kills a running detached command', async () => {
    const client = new MemorySandboxClient({
      commands: {
        hang: ({ signal }) =>
          new Promise((resolve) => signal.addEventListener('abort', () => resolve({ exitCode: 0 }))),
      },
    });
    const { sandbox } = await create(client);
    const cmd = await sandbox.runCommand({ cmd: 'hang', detached: true });
    expect(await client.pollCommand(sandbox.sandboxId, cmd.cmdId)).toEqual({ outcome: 'running' });
    await cmd.kill('kill');
    const finished = await cmd.wait({ pollIntervalMs: 1 });
    expect(finished.exitCode).toBe(137);
    expect(await client.killCommand(sandbox.sandboxId, cmd.cmdId)).toEqual({ outcome: 'already_completed' });
    expect(await client.killCommand(sandbox.sandboxId, 'missing')).toEqual({ outcome: 'not_found' });
  });

  it('validates inputs with zod', async () => {
    const { client, sandbox } = await create();
    await expect(client.runCommand(sandbox.sandboxId, [])).rejects.toMatchObject({ code: 'invalid_request' });
    await expect(client.writeFiles(sandbox.sandboxId, [{ path: '', content: [1] }])).rejects.toBeInstanceOf(
      SandboxWsError,
    );
    await expect(client.writeFiles(sandbox.sandboxId, [{ path: '/x', content: [300] }])).rejects.toMatchObject({
      code: 'invalid_request',
    });
    await expect(Sandbox.create(client, { tenant: tenantId(''), timeoutMs: 1000 })).rejects.toMatchObject({
      code: 'invalid_request',
    });
  });

  it('refuses commands on a released lease and unknown leases', async () => {
    const { client, sandbox } = await create();
    await sandbox.stop();
    await expect(sandbox.runCommand('echo')).rejects.toMatchObject({ code: 'not_found' });
    await expect(client.readFile('nope' as LeaseId, '/x')).rejects.toMatchObject({ code: 'not_found' });
  });

  it('reuses a lease for a repeated idempotency key', async () => {
    const client = new MemorySandboxClient();
    const a = await Sandbox.create(client, { tenant: tenantId('t'), idempotencyKey: 'k' });
    const b = await Sandbox.create(client, { tenant: tenantId('t'), idempotencyKey: 'k' });
    expect(a.sandboxId).toBe(b.sandboxId);
    expect(a.created).toBe(true);
    expect(b.created).toBe(false);
  });

  it('looks up named leases and lists live ones', async () => {
    const client = new MemorySandboxClient();
    const a = await Sandbox.create(client, { tenant: tenantId('t'), name: 'dev' });
    expect((await Sandbox.get(client, { name: 'dev' })).sandboxId).toBe(a.sandboxId);
    expect(await Sandbox.list(client)).toHaveLength(1);
    await a.delete();
    await expect(Sandbox.get(client, { name: 'dev' })).rejects.toThrow('not found');
  });

  it('filters listed leases by tags', async () => {
    const client = new MemorySandboxClient();
    await Sandbox.create(client, { tenant: tenantId('t'), tags: { team: 'a' } });
    await Sandbox.create(client, { tenant: tenantId('t'), tags: { team: 'b' } });
    expect(await Sandbox.list(client, { tags: { team: 'a' } })).toHaveLength(1);
    expect(await Sandbox.list(client)).toHaveLength(2);
  });

  it('preview returns the gateway url and expiry, and rejects a bare endpoint', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('t') });
    client.getSandboxDomain = async () => ({
      outcome: 'available',
      endpoint: 'h:1',
      url: 'https://gw/v1/preview/l/3000/tok/',
      expires_at: 1_700_000_000,
    });
    await expect(sandbox.preview(3000)).resolves.toEqual({
      url: 'https://gw/v1/preview/l/3000/tok/',
      expiresAt: new Date(1_700_000_000_000),
    });
    client.getSandboxDomain = async () => ({ outcome: 'available', endpoint: 'h:1' });
    await expect(sandbox.preview(3000)).rejects.toThrow('not enabled');
  });

  it('preview forwards waitMs and raises a typed retryable timeout', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('t') });
    const seen: Array<number | undefined> = [];
    client.getSandboxDomain = async (_lease, _port, waitMs) => {
      seen.push(waitMs);
      return { outcome: 'not_ready', waited_ms: 1500 };
    };
    const err = await sandbox.preview(3000, { waitMs: 1500 }).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(PreviewNotReadyError);
    expect(err).toMatchObject({ code: 'timeout', retryable: true, port: 3000, waitedMs: 1500 });
    expect(seen).toEqual([1500]);
    await expect(sandbox.preview(3000, { waitMs: -1 })).rejects.toThrow(RangeError);
  });

  it('throws unsupported for verbs it does not model', async () => {
    const { sandbox } = await create();
    await expect(sandbox.createUser('bob')).rejects.toMatchObject({ code: 'unsupported' });
    await expect(sandbox.updateNetworkPolicy('deny-all')).rejects.toMatchObject({ code: 'unsupported' });
    await expect(sandbox.domain(3000)).rejects.toThrow('does not support');
  });
});

describe('MemorySandboxClient launch keys', () => {
  it('runs a keyed command once and replays its reply on retry', async () => {
    const { client, sandbox } = await create();
    const id = sandbox.sandboxId as LeaseId;
    let runs = 0;
    client.registerCommand('tick', () => {
      runs += 1;
      return { exitCode: 0, stdout: `run ${runs}\n`, stderr: '' };
    });
    const first = await client.runCommand(id, ['tick'], { launchKey: 'k-1' });
    const retry = await client.runCommand(id, ['tick'], { launchKey: 'k-1' });
    expect(retry).toEqual(first);
    expect(runs).toBe(1);
    await client.runCommand(id, ['tick']);
    await client.runCommand(id, ['tick']);
    expect(runs).toBe(3);
  });

  it('refuses a key reused for a different command', async () => {
    const { client, sandbox } = await create();
    const id = sandbox.sandboxId as LeaseId;
    await client.runCommand(id, ['echo', 'a'], { launchKey: 'k-2' });
    await expect(client.runCommand(id, ['echo', 'b'], { launchKey: 'k-2' })).rejects.toBeInstanceOf(SandboxWsError);
  });

  it('scopes keys to their lease and refuses a malformed key', async () => {
    const client = new MemorySandboxClient();
    const a = await Sandbox.create(client, { tenant: tenantId('acme'), timeoutMs: 60_000 });
    const b = await Sandbox.create(client, { tenant: tenantId('acme'), timeoutMs: 60_000 });
    let runs = 0;
    client.registerCommand('tick', () => {
      runs += 1;
      return { exitCode: 0, stdout: '', stderr: '' };
    });
    await client.runCommand(a.sandboxId as LeaseId, ['tick'], { launchKey: 'shared' });
    await client.runCommand(b.sandboxId as LeaseId, ['tick'], { launchKey: 'shared' });
    expect(runs).toBe(2);
    await expect(
      client.runCommand(a.sandboxId as LeaseId, ['tick'], { launchKey: 'has spaces' }),
    ).rejects.toMatchObject({ code: 'invalid_request' });
  });
});
