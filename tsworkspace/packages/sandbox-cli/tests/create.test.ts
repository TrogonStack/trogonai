import type { WsFrame } from '@trogonai/sandbox-ws-client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import { runCli } from '../src/cli.js';
import { baseArgv, onRequest, sendResult, startFakeGateway, stopFakeGateway, type FakeGateway } from './harness.js';

describe('sandbox create', () => {
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

  it('constructs a sandbox_lease frame matching the parsed image, tags, and env', async () => {
    let received: Extract<WsFrame, { kind: 'request' }> | undefined;
    onRequest(gateway, 'sandbox_lease', (frame, socket) => {
      received = frame;
      sendResult(socket, frame.id, {
        lease_id: 'lease-created',
        provider: 'mock',
        sandbox_id: 'sandbox-1',
        evidence_subject: 'evidence.subject',
        deadline: { secs_since_epoch: 2000, nanos_since_epoch: 0 },
      });
    });

    const exitCode = await runCli(
      baseArgv(gateway, [
        'create',
        '--image',
        'docker.io/library/alpine:latest',
        '--tag',
        'env=test',
        '--tag',
        'team=platform',
        '--env',
        'FOO=bar',
        '--timeout',
        '5000',
        '--json',
      ]),
    );

    expect(exitCode).toBe(0);
    expect(received?.tool).toBe('sandbox_lease');
    const payload = received?.payload as {
      tenant: string;
      spec: { image: unknown; env: Record<string, string> };
      ttl_seconds: number;
      tags: Record<string, string>;
    };
    expect(payload.tenant).toBe('tenant-a');
    expect(payload.spec.image).toEqual({
      kind: 'registry',
      reference: { registry: 'docker.io', repository: 'library/alpine', tag: 'latest' },
    });
    expect(payload.spec.env).toEqual({ FOO: 'bar' });
    expect(payload.ttl_seconds).toBe(5);
    expect(payload.tags).toEqual({ env: 'test', team: 'platform', client: 'sandbox-ws-client' });

    const printed = stdoutSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printed).toContain('lease-created');
  });

  it('sends retention toggles for --no-monitoring and --no-snapshots', async () => {
    const specs: unknown[] = [];
    onRequest(gateway, 'sandbox_lease', (frame, socket) => {
      specs.push((frame.payload as { spec: unknown }).spec);
      sendResult(socket, frame.id, {
        lease_id: 'lease-created',
        provider: 'mock',
        sandbox_id: 'sandbox-1',
        evidence_subject: 'evidence.subject',
        deadline: { secs_since_epoch: 2000, nanos_since_epoch: 0 },
      });
    });

    const image = ['create', '--image', 'docker.io/library/alpine:latest', '--json'];
    expect(await runCli(baseArgv(gateway, image))).toBe(0);
    expect(await runCli(baseArgv(gateway, [...image, '--no-monitoring', '--no-snapshots']))).toBe(0);

    expect((specs[0] as { retention?: unknown }).retention).toBeUndefined();
    expect((specs[1] as { retention?: unknown }).retention).toEqual({ monitoring: false, snapshots: false });
  });

  it('rejects more than 5 tags before contacting the gateway', async () => {
    let sawRequest = false;
    onRequest(gateway, 'sandbox_lease', () => {
      sawRequest = true;
    });

    const exitCode = await runCli(
      baseArgv(gateway, [
        'create',
        '--image',
        'alpine',
        '--tag',
        'a=1',
        '--tag',
        'b=2',
        '--tag',
        'c=3',
        '--tag',
        'd=4',
        '--tag',
        'e=5',
        '--tag',
        'f=6',
      ]),
    );

    expect(exitCode).toBe(1);
    expect(sawRequest).toBe(false);
    const printedErr = stderrSpy.mock.calls.map((call: unknown[]) => call[0]).join('');
    expect(printedErr).toContain('too many tags');
  });

  it('defaults the registry to docker.io when the image has no registry-looking prefix', async () => {
    let received: Extract<WsFrame, { kind: 'request' }> | undefined;
    onRequest(gateway, 'sandbox_lease', (frame, socket) => {
      received = frame;
      sendResult(socket, frame.id, {
        lease_id: 'lease-2',
        provider: 'mock',
        sandbox_id: 'sandbox-2',
        evidence_subject: 'evidence.subject',
        deadline: { secs_since_epoch: 1000, nanos_since_epoch: 0 },
      });
    });

    const exitCode = await runCli(baseArgv(gateway, ['create', '--image', 'alpine:3.20']));
    expect(exitCode).toBe(0);
    const payload = received?.payload as {
      spec: { image: { reference: { registry: string; repository: string; tag: string } } };
    };
    expect(payload.spec.image.reference).toEqual({ registry: 'docker.io', repository: 'alpine', tag: '3.20' });
  });
});
