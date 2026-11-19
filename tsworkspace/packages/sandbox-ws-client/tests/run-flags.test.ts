import { describe, expect, it } from 'vitest';

import { Command, CommandFinished, MemorySandboxClient, Sandbox, tenantId } from '../src/index.ts';
import type { LeaseId } from '../src/index.ts';

describe('run command flags', () => {
  it('carries timed_out and truncated from a completed reply onto CommandFinished', () => {
    const client = new MemorySandboxClient();
    const done = Command.fromRunCommandResponse(client, 'l' as LeaseId, {
      outcome: 'completed',
      exit_code: -1,
      stdout: [104, 105],
      stderr: [],
      timed_out: true,
      truncated: true,
    });
    expect(done).toBeInstanceOf(CommandFinished);
    expect((done as CommandFinished).timedOut).toBe(true);
    expect((done as CommandFinished).truncated).toBe(true);
  });

  it('defaults both flags to false when the worker omits them', () => {
    const done = Command.fromRunCommandResponse(new MemorySandboxClient(), 'l' as LeaseId, {
      outcome: 'completed',
      exit_code: 0,
      stdout: [],
      stderr: [],
    }) as CommandFinished;
    expect(done.timedOut).toBe(false);
    expect(done.truncated).toBe(false);
  });

  it('forwards maxOutputBytes and process to the client as wire fields', async () => {
    const seen: unknown[] = [];
    const client = new MemorySandboxClient();
    const original = client.runCommand.bind(client);
    client.runCommand = (async (leaseId, command, opts) => {
      seen.push(opts);
      return original(leaseId, command, opts);
    }) as typeof client.runCommand;
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    await sandbox.runCommand({ cmd: 'true', detached: true, maxOutputBytes: 2048, process: { name: 'web' } });
    expect(seen[0]).toMatchObject({ maxOutputBytes: 2048, process: { name: 'web' } });
  });
});
