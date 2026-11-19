import { describe, expect, it } from 'vitest';

import { CommandFailedError, SandboxWsError } from '../src/errors.ts';
import { leaseId, tenantId } from '../src/ids.ts';
import { MemorySandboxClient } from '../src/memory.ts';
import { CommandFinished, Sandbox } from '../src/sandbox.ts';

const enc = (text: string) => new TextEncoder().encode(text);
const finished = (exit: number, out = '', err = '', flags = {}) =>
  new CommandFinished(leaseId('l1'), 'c1', exit, enc(out), enc(err), flags);

describe('CommandFinished ergonomics', () => {
  it('ok is true only for exit 0 without a timeout', () => {
    expect(finished(0).ok).toBe(true);
    expect(finished(1).ok).toBe(false);
    expect(finished(0, '', '', { timedOut: true }).ok).toBe(false);
  });

  it('text strips one trailing newline', () => {
    expect(finished(0, 'hi\n').text()).toBe('hi');
    expect(finished(0, 'hi\r\n').text()).toBe('hi');
    expect(finished(0, 'hi\n\n').text()).toBe('hi\n');
    expect(finished(0, '').text()).toBe('');
  });

  it('lines drops empty lines and carriage returns', () => {
    expect(finished(0, 'a\r\n\nb\nc').lines()).toEqual(['a', 'b', 'c']);
    expect(finished(0, '').lines()).toEqual([]);
  });

  it('json parses stdout and throws on garbage', () => {
    expect(finished(0, '{"a":[1,2]}\n').json<{ a: number[] }>()).toEqual({ a: [1, 2] });
    expect(() => finished(0, 'nope').json()).toThrow(SyntaxError);
  });

  it('throwIfFailed returns itself on success', () => {
    const ok = finished(0, 'x');
    expect(ok.throwIfFailed()).toBe(ok);
  });

  it('throwIfFailed raises a typed error with exit code and stderr', () => {
    const err = (() => {
      try {
        finished(2, '', 'boom\n').throwIfFailed();
      } catch (e) {
        return e;
      }
    })();
    expect(err).toBeInstanceOf(CommandFailedError);
    expect(err).toBeInstanceOf(SandboxWsError);
    const failed = err as CommandFailedError;
    expect(failed.exitCode).toBe(2);
    expect(failed.stderr).toBe('boom\n');
    expect(failed.retryable).toBe(false);
    expect(failed.leaseId).toBe('l1');
    expect(failed.message).toContain('code 2');
    expect(failed.message).toContain('boom');
  });

  it('throwIfFailed names a timeout', () => {
    expect(() => finished(-1, '', '', { timedOut: true }).throwIfFailed()).toThrow(/timed out/);
  });

  it('carries truncated through', () => {
    expect(finished(0, '', '', { truncated: true }).truncated).toBe(true);
  });
});

describe('Sandbox.runCommand duration', () => {
  it('sets a non-negative durationMs on a synchronous run', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    const result = await sandbox.runCommand('echo', ['hi']);
    expect(result.ok).toBe(true);
    expect(result.durationMs).toBeGreaterThanOrEqual(0);
    expect(result.text()).toBe('hi');
  });

  it('leaves durationMs unset for a waited detached command', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    const cmd = await sandbox.runCommand({ cmd: 'echo', args: ['hi'], detached: true });
    const done = await cmd.wait();
    expect(done.durationMs).toBeUndefined();
  });
});
