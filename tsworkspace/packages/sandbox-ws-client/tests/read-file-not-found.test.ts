import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { tenantId } from '../src/ids.ts';
import { MemorySandboxClient } from '../src/memory.ts';
import { Sandbox } from '../src/sandbox.ts';
import type { ReadFileResponse } from '../src/wire.ts';
import { connectTestClient, onRequest, respond, startFakeServer, stopFakeServer } from './harness.ts';
import type { ServerContext } from './harness.ts';

class ScriptedClient extends MemorySandboxClient {
  constructor(private readonly script: () => Promise<ReadFileResponse>) {
    super();
  }
  override async readFile(): Promise<ReadFileResponse> {
    return this.script();
  }
}

describe('readFile not found', () => {
  let ctx: ServerContext;
  beforeEach(async () => {
    ctx = await startFakeServer();
  });
  afterEach(async () => {
    await stopFakeServer(ctx);
  });

  it('passes the typed not_found outcome through the ws client', async () => {
    onRequest(ctx, (frame, socket) => respond(socket, frame, { outcome: 'not_found' }));
    const client = await connectTestClient(ctx);
    const response = await client.readFile('lease-1' as never, '/missing');
    expect(response).toEqual({ outcome: 'not_found' });
    client.close();
  });

  it('returns null for the typed outcome', async () => {
    const client = new ScriptedClient(async () => ({ outcome: 'not_found' }));
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    expect(await sandbox.readFile({ path: '/missing' })).toBeNull();
  });

  it('still returns null for an older gateway that fails with the worker message', async () => {
    const client = new ScriptedClient(async () => {
      throw new Error('cat: /missing: No such file or directory');
    });
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    expect(await sandbox.readFile({ path: '/missing' })).toBeNull();
  });

  it('rethrows unrelated failures', async () => {
    const client = new ScriptedClient(async () => {
      throw new Error('connection reset');
    });
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    await expect(sandbox.readFile({ path: '/x' })).rejects.toThrow('connection reset');
  });
});
