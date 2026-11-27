import type { WsFrame } from '@trogonai/sandbox-ws-client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { runCli } from '../src/cli.js';
import { baseArgv, onRequest, sendResult, startFakeGateway, stopFakeGateway, type FakeGateway } from './harness.js';

describe('sandbox kill', () => {
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

  it('sends a sandbox_kill_command frame with the default term signal', async () => {
    let received: Extract<WsFrame, { kind: 'request' }> | undefined;
    onRequest(gateway, 'sandbox_kill_command', (frame, socket) => {
      received = frame;
      sendResult(socket, frame.id, { outcome: 'signaled' });
    });

    const exitCode = await runCli(baseArgv(gateway, ['kill', 'lease-1', 'exec-1']));

    expect(exitCode).toBe(0);
    expect(received?.payload).toEqual({ lease_id: 'lease-1', execution_id: 'exec-1', signal: 'term' });
    const printed = stdoutSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printed).toContain('signaled');
  });

  it('passes through a non-default --signal', async () => {
    let received: Extract<WsFrame, { kind: 'request' }> | undefined;
    onRequest(gateway, 'sandbox_kill_command', (frame, socket) => {
      received = frame;
      sendResult(socket, frame.id, { outcome: 'signaled' });
    });

    await runCli(baseArgv(gateway, ['kill', 'lease-1', 'exec-1', '--signal', 'kill']));

    expect(received?.payload).toEqual({ lease_id: 'lease-1', execution_id: 'exec-1', signal: 'kill' });
  });

  it('rejects an invalid --signal value client-side', async () => {
    let sawRequest = false;
    onRequest(gateway, 'sandbox_kill_command', () => {
      sawRequest = true;
    });

    const exitCode = await runCli(baseArgv(gateway, ['kill', 'lease-1', 'exec-1', '--signal', 'bogus']));

    expect(exitCode).toBe(1);
    expect(sawRequest).toBe(false);
  });
});
