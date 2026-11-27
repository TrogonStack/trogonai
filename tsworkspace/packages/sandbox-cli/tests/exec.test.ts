import type { WsFrame } from '@trogonai/sandbox-ws-client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { runCli } from '../src/cli.js';
import { baseArgv, onRequest, sendResult, startFakeGateway, stopFakeGateway, type FakeGateway } from './harness.js';

describe('sandbox exec', () => {
  let gateway: FakeGateway;
  let stdoutSpy: ReturnType<typeof vi.spyOn>;
  let stderrSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(async () => {
    gateway = await startFakeGateway();
    stdoutSpy = vi.spyOn(process.stdout, 'write').mockImplementation(() => true);
    stderrSpy = vi.spyOn(process.stderr, 'write').mockImplementation(() => true);
  });

  afterEach(async () => {
    stdoutSpy.mockRestore();
    stderrSpy.mockRestore();
    await stopFakeGateway(gateway);
  });

  it('propagates a zero exit code and prints stdout on success', async () => {
    let received: Extract<WsFrame, { kind: 'request' }> | undefined;
    onRequest(gateway, 'sandbox_run_command', (frame, socket) => {
      received = frame;
      sendResult(socket, frame.id, {
        outcome: 'completed',
        exit_code: 0,
        stdout: Array.from(Buffer.from('hello\n')),
        stderr: [],
      });
    });

    const exitCode = await runCli(baseArgv(gateway, ['exec', 'lease-1', '--', 'echo', 'hi']));

    expect(exitCode).toBe(0);
    const payload = received?.payload as { command: string[]; lease_id: string };
    expect(payload.command).toEqual(['echo', 'hi']);
    expect(payload.lease_id).toBe('lease-1');
    const printed = stdoutSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printed).toContain('hello');
  });

  it('propagates a nonzero exit code and prints stderr', async () => {
    onRequest(gateway, 'sandbox_run_command', (frame, socket) => {
      sendResult(socket, frame.id, {
        outcome: 'completed',
        exit_code: 7,
        stdout: [],
        stderr: Array.from(Buffer.from('boom\n')),
      });
    });

    const exitCode = await runCli(baseArgv(gateway, ['exec', 'lease-1', '--', 'false']));

    expect(exitCode).toBe(7);
    const printedErr = stderrSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printedErr).toContain('boom');
  });

  it('errors clearly when no remote command is given after --', async () => {
    const exitCode = await runCli(baseArgv(gateway, ['exec', 'lease-1']));
    expect(exitCode).toBe(1);
    const printedErr = stderrSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printedErr).toContain('missing remote command');
  });

  it('prints the execution id and exits 0 when --detached is set', async () => {
    onRequest(gateway, 'sandbox_run_command', (frame, socket) => {
      sendResult(socket, frame.id, { outcome: 'started', execution_id: 'exec-123' });
    });

    const exitCode = await runCli(baseArgv(gateway, ['exec', 'lease-1', '--detached', '--', 'sleep', '10']));

    expect(exitCode).toBe(0);
    const printed = stdoutSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printed).toContain('exec-123');
  });
});
