import type { WsFrame } from '@trogonai/sandbox-ws-client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { runCli } from '../src/cli.js';
import { baseArgv, onRequest, sendResult, startFakeGateway, stopFakeGateway, type FakeGateway } from './harness.js';

const DIGEST = `sha256:${'a'.repeat(64)}`;

describe('sandbox tags set', () => {
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

  it('sends a sandbox_set_snapshot_tag frame and prints confirmation on applied', async () => {
    let received: Extract<WsFrame, { kind: 'request' }> | undefined;
    onRequest(gateway, 'sandbox_set_snapshot_tag', (frame, socket) => {
      received = frame;
      sendResult(socket, frame.id, { outcome: 'applied' });
    });

    const exitCode = await runCli(baseArgv(gateway, ['tags', 'set', 'latest', DIGEST]));

    expect(exitCode).toBe(0);
    expect(received?.payload).toEqual({ name: 'latest', digest: DIGEST });
    const printed = stdoutSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printed).toContain('latest');
    expect(printed).toContain(DIGEST);
  });

  it('exits nonzero when denied', async () => {
    onRequest(gateway, 'sandbox_set_snapshot_tag', (frame, socket) => {
      sendResult(socket, frame.id, { outcome: 'denied' });
    });

    const exitCode = await runCli(baseArgv(gateway, ['tags', 'set', 'latest', DIGEST]));

    expect(exitCode).toBe(1);
    const printedErr = stderrSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printedErr).toContain('denied');
  });
});
