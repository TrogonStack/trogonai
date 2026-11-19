import { afterEach, describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';
import type { CommandContext } from '../src/index.ts';

const tenant = tenantId('acme');
const endings: Array<() => void> = [];

afterEach(async () => {
  for (const end of endings.splice(0)) end();
  await new Promise((resolve) => setTimeout(resolve, 0));
});

async function setup(env?: Record<string, string>) {
  const starts: Array<Pick<CommandContext, 'args' | 'cwd' | 'env'>> = [];
  const client = new MemorySandboxClient({
    commands: {
      serve: (context) => {
        starts.push({ args: [...context.args], cwd: context.cwd, env: { ...context.env } });
        return new Promise((resolve) => {
          const end = () => resolve({ exitCode: 137 });
          endings.push(end);
          context.signal.addEventListener('abort', end, { once: true });
        });
      },
      sh: () => ({ exitCode: 0, stdout: '200' }),
    },
  });
  client.waitSandboxReady = async () => ({ outcome: 'ready' });
  client.getSandboxDomain = async (id) => ({
    outcome: 'available',
    endpoint: 'preview:3000',
    url: `https://preview.example/${id}/`,
    expires_at: 1_900_000_000,
  });
  const sandbox = await Sandbox.create(client, { tenant, env });
  return { client, sandbox, starts };
}

type Receipt = Parameters<typeof Sandbox.fromSnapshot>[1] & {
  clientState?: { version: number; sourceSandboxId: string; servers: unknown[] };
};

async function serializedSnapshot(sandbox: Sandbox): Promise<Receipt> {
  return JSON.parse(JSON.stringify(await sandbox.snapshot())) as Receipt;
}

describe('server restoration across handles', () => {
  it('automatically restarts registered servers from a serialized snapshot receipt and waits for readiness', async () => {
    const { sandbox, client, starts } = await setup();
    await sandbox.startServer({
      cmd: 'serve',
      args: ['--dev'],
      cwd: '/work',
      port: 3000,
      path: '/healthz',
      name: 'web',
    });
    const receipt = await serializedSnapshot(sandbox);
    const checked: number[][] = [];
    client.waitSandboxReady = async (_id, ports) => {
      checked.push(ports ?? []);
      return { outcome: 'ready' };
    };
    const restored = await Sandbox.fromSnapshot(client, receipt, { tenant });
    expect(starts.map(({ args, cwd }) => ({ args, cwd }))).toEqual([
      { args: ['--dev'], cwd: '/work' },
      { args: ['--dev'], cwd: '/work' },
    ]);
    expect(checked).toContainEqual([3000]);
    expect((await restored.preview(3000)).url).toContain(restored.sandboxId);
  });

  it('omits servers opted out of restart and servers explicitly stopped', async () => {
    const { sandbox, client, starts } = await setup();
    await sandbox.startServer({ cmd: 'serve', port: 3000, restartOnResume: false });
    const stopped = await sandbox.startServer({ cmd: 'serve', port: 3001 });
    await stopped.stop();
    await Sandbox.fromSnapshot(client, await serializedSnapshot(sandbox), { tenant });
    expect(starts).toHaveLength(2);
  });

  it('records environment names without values and uses the restoring callers replacements', async () => {
    const { sandbox, client, starts } = await setup({ APP_CONFIG: 'original-config-value' });
    await sandbox.startServer({
      cmd: 'serve',
      port: 3000,
      env: { SESSION_TOKEN: 'original-token-value' },
      redactValues: ['original-token-value'],
    });
    const receipt = await serializedSnapshot(sandbox);
    expect(JSON.stringify(receipt)).not.toContain('original-config-value');
    expect(JSON.stringify(receipt)).not.toContain('original-token-value');
    await Sandbox.fromSnapshot(client, receipt, {
      tenant,
      env: { APP_CONFIG: 'replacement-config', SESSION_TOKEN: 'replacement-token' },
    });
    expect(starts.at(-1)?.env).toMatchObject({ APP_CONFIG: 'replacement-config', SESSION_TOKEN: 'replacement-token' });
    expect(starts).toHaveLength(2);
  });

  it('rejects missing replacement environment before requesting a restore lease', async () => {
    const { sandbox, client } = await setup();
    await sandbox.startServer({ cmd: 'serve', port: 3000, env: { SESSION_TOKEN: 'original-token-value' } });
    const receipt = await serializedSnapshot(sandbox);
    let leased = false;
    client.lease = async () => {
      leased = true;
      throw new Error('should not lease');
    };
    await expect(Sandbox.fromSnapshot(client, receipt, { tenant })).rejects.toMatchObject({ code: 'invalid_request' });
    expect(leased).toBe(false);
  });

  it('does not serialize a registered redaction value embedded in restart command arguments', async () => {
    const { sandbox, client } = await setup();
    await sandbox.startServer({
      cmd: 'serve',
      args: ['--token', 'original-token-value'],
      port: 3000,
      redactValues: ['original-token-value'],
    });
    let captured = false;
    client.snapshotLease = async () => {
      captured = true;
      return { outcome: 'unsupported' };
    };
    await expect(sandbox.snapshot()).rejects.toMatchObject({ code: 'invalid_request' });
    expect(captured).toBe(false);
  });

  it('repopulates a reattached handle from explicit state and restarts gone processes on resume', async () => {
    const { sandbox, client, starts } = await setup();
    const server = await sandbox.startServer({ cmd: 'serve', port: 3000, name: 'web' });
    const receipt = await serializedSnapshot(sandbox);
    const attached = await Sandbox.connect(client, sandbox.sandboxId, { clientState: receipt.clientState });
    await attached.resume();
    expect(starts).toHaveLength(1);
    await server.command.kill('kill');
    await attached.resume();
    expect(starts).toHaveLength(2);
  });

  it('does not turn a guest-authored file into restart authority', async () => {
    const { sandbox, client, starts } = await setup();
    await sandbox.writeFiles([
      { path: '/work/.server-state.json', content: JSON.stringify({ servers: [{ cmd: 'serve', port: 3000 }] }) },
    ]);
    await Sandbox.fromSnapshot(client, await serializedSnapshot(sandbox), { tenant });
    expect(starts).toHaveLength(0);
  });

  it('reattaches a named process preserved by a native snapshot instead of starting a duplicate', async () => {
    const { sandbox, client, starts } = await setup();
    await sandbox.startServer({ cmd: 'serve', port: 3000, name: 'web' });
    const captured = await serializedSnapshot(sandbox);
    const receipt = {
      reference: { provider: 'e2b' as const, id: 'native-snapshot' },
      bytes: 0,
      clientState: captured.clientState,
    };
    const lease = client.lease.bind(client);
    client.lease = async (request) => ({
      ...(await lease({
        ...request,
        spec: { ...request.spec, image: { kind: 'snapshot', reference: captured.reference as never } },
      })),
      provider: 'e2b',
    });
    let preserved = true;
    let listed = false;
    client.listProcesses = async () => {
      listed = true;
      return { outcome: 'processes', processes: [{ execution_id: 'preserved-web', name: 'web', state: 'running' }] };
    };
    const poll = client.pollCommand.bind(client);
    client.pollCommand = async (id, execution) =>
      execution === 'preserved-web'
        ? preserved
          ? { outcome: 'running' }
          : { outcome: 'not_found' }
        : poll(id, execution);
    const restored = await Sandbox.fromSnapshot(client, receipt, { tenant });
    expect(starts).toHaveLength(1);
    expect(listed).toBe(true);
    preserved = false;
    await restored.resume();
    expect(starts).toHaveLength(2);
  });

  it.each([
    { version: 99, sourceSandboxId: 'source', servers: [] },
    { version: 1, sourceSandboxId: 'source', servers: [], guestAuthority: true },
    {
      version: 1,
      sourceSandboxId: 'source',
      servers: Array.from({ length: 33 }, () => ({ cmd: 'serve', port: 3000 })),
    },
    { version: 1, sourceSandboxId: 'source', servers: [{ cmd: 'x'.repeat(70_000), port: 3000 }] },
  ])('rejects malformed or unbounded restart state before leasing', async (clientState) => {
    const { sandbox, client } = await setup();
    const receipt = { ...(await serializedSnapshot(sandbox)), clientState } as Receipt;
    let leased = false;
    client.lease = async () => {
      leased = true;
      throw new Error('should not lease');
    };
    await expect(Sandbox.fromSnapshot(client, receipt, { tenant })).rejects.toMatchObject({ code: 'invalid_request' });
    expect(leased).toBe(false);
  });

  it('keeps old receipts without restart state compatible', async () => {
    const { sandbox, client, starts } = await setup();
    await sandbox.writeFiles([{ path: '/work/a.txt', content: 'hello' }]);
    const original = await serializedSnapshot(sandbox);
    const legacy = 'bytes' in original ? { reference: original.reference, bytes: original.bytes } : original;
    const restored = await Sandbox.fromSnapshot(client, legacy, { tenant });
    expect(await restored.readFile({ path: '/work/a.txt' })).toBe('hello');
    expect(starts).toHaveLength(0);
  });
});
