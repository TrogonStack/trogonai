import * as fs from 'node:fs';
import * as os from 'node:os';
import * as path from 'node:path';

import type { WsFrame } from '@trogonai/sandbox-ws-client';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { runCli } from '../src/cli.js';
import { baseArgv, onRequest, sendResult, startFakeGateway, stopFakeGateway, type FakeGateway } from './harness.js';

describe('sandbox copy', () => {
  let gateway: FakeGateway;
  let tmpDir: string;

  beforeEach(async () => {
    gateway = await startFakeGateway();
    tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'sandbox-cli-copy-'));
  });

  afterEach(async () => {
    await stopFakeGateway(gateway);
    fs.rmSync(tmpDir, { recursive: true, force: true });
  });

  it('copies a local file to a remote path via sandbox_write_files', async () => {
    const localPath = path.join(tmpDir, 'local.txt');
    fs.writeFileSync(localPath, 'hello remote');

    let received: Extract<WsFrame, { kind: 'request' }> | undefined;
    onRequest(gateway, 'sandbox_write_files', (frame, socket) => {
      received = frame;
      sendResult(socket, frame.id, { outcome: 'written' });
    });

    const exitCode = await runCli(baseArgv(gateway, ['copy', localPath, 'lease-1:/workspace/remote.txt']));

    expect(exitCode).toBe(0);
    const payload = received?.payload as {
      lease_id: string;
      files: Array<{ path: string; content: number[] }>;
    };
    expect(payload.lease_id).toBe('lease-1');
    expect(payload.files).toHaveLength(1);
    expect(payload.files[0].path).toBe('/workspace/remote.txt');
    expect(Buffer.from(payload.files[0].content).toString('utf8')).toBe('hello remote');
  });

  it('copies a remote file to a local path via sandbox_read_file', async () => {
    onRequest(gateway, 'sandbox_read_file', (frame, socket) => {
      sendResult(socket, frame.id, { outcome: 'content', data: Array.from(Buffer.from('hello local')) });
    });

    const localPath = path.join(tmpDir, 'downloaded.txt');
    const exitCode = await runCli(baseArgv(gateway, ['copy', 'lease-1:/workspace/remote.txt', localPath]));

    expect(exitCode).toBe(0);
    expect(fs.readFileSync(localPath, 'utf8')).toBe('hello local');
  });

  it('rejects remote-to-remote copies', async () => {
    const exitCode = await runCli(baseArgv(gateway, ['copy', 'lease-1:/a', 'lease-2:/b']));
    expect(exitCode).toBe(1);
  });

  it('rejects local-to-local copies', async () => {
    const a = path.join(tmpDir, 'a.txt');
    const b = path.join(tmpDir, 'b.txt');
    fs.writeFileSync(a, 'x');
    const exitCode = await runCli(baseArgv(gateway, ['copy', a, b]));
    expect(exitCode).toBe(1);
  });
});
