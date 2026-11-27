import type { WsFrame } from '@trogonai/sandbox-ws-client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { runCli } from '../src/cli.js';
import { baseArgv, onRequest, sendResult, startFakeGateway, stopFakeGateway, type FakeGateway } from './harness.js';

describe('sandbox fs exists', () => {
  let gateway: FakeGateway;
  let stdoutSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(async () => {
    gateway = await startFakeGateway();
    stdoutSpy = vi.spyOn(process.stdout, 'write').mockImplementation(() => true);
  });

  afterEach(async () => {
    stdoutSpy.mockRestore();
    await stopFakeGateway(gateway);
  });

  it('sends a `test -e <path>` run_command frame and exits 0 when it exists', async () => {
    let received: Extract<WsFrame, { kind: 'request' }> | undefined;
    onRequest(gateway, 'sandbox_run_command', (frame, socket) => {
      received = frame;
      sendResult(socket, frame.id, { outcome: 'completed', exit_code: 0, stdout: [], stderr: [] });
    });

    const exitCode = await runCli(baseArgv(gateway, ['fs', 'exists', 'lease-1', '/workspace/file.txt']));

    expect(exitCode).toBe(0);
    const payload = received?.payload as { command: string[]; lease_id: string };
    expect(payload.command).toEqual(['test', '-e', '/workspace/file.txt']);
    expect(payload.lease_id).toBe('lease-1');
    const printed = stdoutSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printed).toContain('true');
  });

  it('exits 1 when the path does not exist', async () => {
    onRequest(gateway, 'sandbox_run_command', (frame, socket) => {
      sendResult(socket, frame.id, { outcome: 'completed', exit_code: 1, stdout: [], stderr: [] });
    });

    const exitCode = await runCli(baseArgv(gateway, ['fs', 'exists', 'lease-1', '/nope']));

    expect(exitCode).toBe(1);
    const printed = stdoutSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printed).toContain('false');
  });
});
