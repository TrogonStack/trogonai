import { describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, SandboxWsError, TimeoutError, tenantId } from '../src/index.ts';
import type { WaitSandboxReadyResponse } from '../src/index.ts';

function scripted(client: MemorySandboxClient, replies: WaitSandboxReadyResponse[]) {
  const calls: Array<{ ports?: number[]; seconds?: number }> = [];
  client.waitSandboxReady = (async (_lease, ports, seconds) => {
    calls.push({ ports, seconds });
    return replies.shift() ?? { outcome: 'ready' };
  }) as typeof client.waitSandboxReady;
  return calls;
}

describe('Sandbox.waitUntilReady', () => {
  it('resolves once the broker reports ready and sends the ports', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    const calls = scripted(client, [{ outcome: 'not_ready', pending_ports: [3000] }, { outcome: 'ready' }]);
    await sandbox.waitUntilReady({ ports: [3000], timeoutMs: 60_000 });
    expect(calls).toHaveLength(2);
    expect(calls[0]?.ports).toEqual([3000]);
    expect(calls[0]?.seconds).toBeLessThanOrEqual(20);
  });

  it('throws a TimeoutError naming the ports that never answered', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    scripted(client, [{ outcome: 'not_ready', pending_ports: [3000] }]);
    const failure = sandbox.waitUntilReady({ ports: [3000], timeoutMs: 0 });
    await expect(failure).rejects.toBeInstanceOf(TimeoutError);
    await expect(failure).rejects.toThrow(/3000/);
  });

  it('throws unsupported when the provider cannot check ports', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    await expect(sandbox.waitUntilReady({ ports: [3000] })).rejects.toMatchObject({ code: 'unsupported' });
  });

  it('refuses more ports than the broker accepts without a round trip', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    const calls = scripted(client, []);
    const ports = Array.from({ length: 17 }, (_, i) => 1000 + i);
    await expect(sandbox.waitUntilReady({ ports })).rejects.toBeInstanceOf(SandboxWsError);
    expect(calls).toHaveLength(0);
  });

  it('resolves for a running sandbox with no ports', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    await sandbox.waitUntilReady();
  });
});
