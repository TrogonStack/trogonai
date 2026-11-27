import type { WsFrame } from '@trogonai/sandbox-ws-client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { runCli } from '../src/cli.js';
import { baseArgv, onRequest, sendResult, startFakeGateway, stopFakeGateway, type FakeGateway } from './harness.js';

describe('sandbox get', () => {
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

  it('sends a sandbox_get_lease_by_name frame and prints the row on success', async () => {
    let received: Extract<WsFrame, { kind: 'request' }> | undefined;
    onRequest(gateway, 'sandbox_get_lease_by_name', (frame, socket) => {
      received = frame;
      sendResult(socket, frame.id, {
        outcome: 'found',
        row: {
          lease_id: 'lease-1',
          tenant: 'tenant-a',
          provider: 'mock',
          status: 'running',
          created_at: { secs_since_epoch: 1000, nanos_since_epoch: 0 },
          deadline: { secs_since_epoch: 2000, nanos_since_epoch: 0 },
          name: 'my-sandbox',
        },
      });
    });

    const exitCode = await runCli(baseArgv(gateway, ['get', 'my-sandbox']));

    expect(exitCode).toBe(0);
    expect(received?.payload).toEqual({ name: 'my-sandbox' });
    const printed = stdoutSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printed).toContain('lease-1');
    expect(printed).toContain('my-sandbox');
  });

  it('exits nonzero when not found', async () => {
    onRequest(gateway, 'sandbox_get_lease_by_name', (frame, socket) => {
      sendResult(socket, frame.id, { outcome: 'not_found' });
    });

    const exitCode = await runCli(baseArgv(gateway, ['get', 'missing']));

    expect(exitCode).toBe(1);
    const printedErr = stderrSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printedErr).toContain('not found');
  });
});
