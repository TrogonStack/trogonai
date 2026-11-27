import type { WsFrame } from '@trogonai/sandbox-ws-client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { runCli } from '../src/cli.js';
import { baseArgv, onRequest, sendResult, startFakeGateway, stopFakeGateway, type FakeGateway } from './harness.js';

describe('sandbox domain', () => {
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

  it('sends a sandbox_get_domain frame and prints the endpoint on success', async () => {
    let received: Extract<WsFrame, { kind: 'request' }> | undefined;
    onRequest(gateway, 'sandbox_get_domain', (frame, socket) => {
      received = frame;
      sendResult(socket, frame.id, { outcome: 'available', endpoint: 'lease-1-3000.example.com:443' });
    });

    const exitCode = await runCli(baseArgv(gateway, ['domain', 'lease-1', '3000']));

    expect(exitCode).toBe(0);
    expect(received?.payload).toEqual({ lease_id: 'lease-1', port: 3000 });
    const printed = stdoutSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printed).toContain('lease-1-3000.example.com:443');
  });

  it('exits nonzero when the port is not exposed', async () => {
    onRequest(gateway, 'sandbox_get_domain', (frame, socket) => {
      sendResult(socket, frame.id, { outcome: 'port_not_exposed' });
    });

    const exitCode = await runCli(baseArgv(gateway, ['domain', 'lease-1', '3000']));

    expect(exitCode).toBe(1);
    const printedErr = stderrSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printedErr).toContain('not exposed');
  });
});
