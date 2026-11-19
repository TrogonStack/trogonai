import { describe, expect, it } from 'vitest';

import { MemorySandboxClient, NON_INTERACTIVE_ENV, Sandbox, tenantId, withNonInteractiveEnv } from '../src/index.ts';

async function envSeenByCommand(
  params: Parameters<Sandbox['runCommand']>[0] & object,
): Promise<Record<string, string>> {
  const client = new MemorySandboxClient();
  let seen: Record<string, string> = {};
  client.registerCommand('probe', (ctx) => {
    seen = ctx.env;
    return { exitCode: 0, stdout: '', stderr: '' };
  });
  const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
  await sandbox.runCommand(params);
  return seen;
}

describe('non-interactive env defaults', () => {
  it('lists the hang-preventing variables', () => {
    expect(NON_INTERACTIVE_ENV).toMatchObject({
      CI: '1',
      DEBIAN_FRONTEND: 'noninteractive',
      GIT_TERMINAL_PROMPT: '0',
    });
  });

  it('reaches a command that sets no env of its own', async () => {
    const env = await envSeenByCommand({ cmd: 'probe' });
    expect(env).toMatchObject({ CI: '1', DEBIAN_FRONTEND: 'noninteractive', GIT_TERMINAL_PROMPT: '0' });
  });

  it('lets the caller win per variable and keeps the rest', async () => {
    const env = await envSeenByCommand({ cmd: 'probe', env: { CI: '0', FOO: 'bar' } });
    expect(env).toMatchObject({ CI: '0', FOO: 'bar', GIT_TERMINAL_PROMPT: '0' });
  });

  it('sends exactly the caller env when opted out', async () => {
    const env = await envSeenByCommand({ cmd: 'probe', env: { FOO: 'bar' }, nonInteractive: false });
    expect(env.CI).toBeUndefined();
    expect(env.GIT_TERMINAL_PROMPT).toBeUndefined();
    expect(env.FOO).toBe('bar');
  });

  it('skips the defaults for an interactive stdin run', () => {
    expect(withNonInteractiveEnv(undefined, { stdin: 'interactive' })).toBeUndefined();
    expect(withNonInteractiveEnv({ A: '1' }, { stdin: 'interactive' })).toEqual({ A: '1' });
  });

  it('never mutates the shared defaults', () => {
    const merged = withNonInteractiveEnv({ CI: '0' });
    expect(merged?.CI).toBe('0');
    expect(NON_INTERACTIVE_ENV.CI).toBe('1');
  });
});
