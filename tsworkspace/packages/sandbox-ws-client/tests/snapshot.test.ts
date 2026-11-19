import { describe, expect, it } from 'vitest';

import { MEMORY_CAPABILITIES, MemorySandboxClient, Sandbox, SandboxWsError, tenantId } from '../src/index.ts';

const tenant = tenantId('acme');

describe('snapshot ergonomics', () => {
  it('declares snapshot verbs as supported', () => {
    expect(MEMORY_CAPABILITIES.supported).toContain('snapshot_lease');
    expect(MEMORY_CAPABILITIES.supported).toContain('snapshot_delete');
  });

  it('snapshots a sandbox and starts another from the receipt', async () => {
    const client = new MemorySandboxClient();
    const source = await Sandbox.create(client, { tenant, timeoutMs: 60_000 });
    await source.writeFiles([{ path: '/work/a.txt', content: 'hello' }]);
    const receipt = await source.snapshot();
    expect(receipt.reference.registry).toBe('memory');
    expect(receipt.bytes).toBe(5);

    await source.writeFiles([{ path: '/work/a.txt', content: 'changed' }]);
    const restored = await Sandbox.fromSnapshot(client, receipt, { tenant, timeoutMs: 60_000 });
    expect(restored.sandboxId).not.toBe(source.sandboxId);
    expect(await restored.readFile({ path: '/work/a.txt' })).toBe('hello');
  });

  it('deletes a snapshot once and reports a repeat as false', async () => {
    const client = new MemorySandboxClient();
    const source = await Sandbox.create(client, { tenant, timeoutMs: 60_000 });
    const receipt = await source.snapshot();
    expect(await Sandbox.deleteSnapshot(client, receipt.reference.digest)).toBe(true);
    expect(await Sandbox.deleteSnapshot(client, receipt.reference.digest)).toBe(false);
    await expect(Sandbox.fromSnapshot(client, receipt, { tenant, timeoutMs: 60_000 })).rejects.toMatchObject({
      code: 'lease_denied',
    });
  });

  it('rejects fromSnapshot combined with image', async () => {
    const client = new MemorySandboxClient();
    const source = await Sandbox.create(client, { tenant, timeoutMs: 60_000 });
    const receipt = await source.snapshot();
    await expect(
      Sandbox.create(client, { tenant, timeoutMs: 60_000, image: 'alpine', fromSnapshot: receipt }),
    ).rejects.toMatchObject({ code: 'invalid_request' });
  });

  it('maps unsupported broker answers to typed errors', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant, timeoutMs: 60_000 });
    client.snapshotLease = async () => ({ outcome: 'unsupported' });
    client.snapshotDelete = async () => ({ outcome: 'unsupported' });
    client.pauseLease = async () => ({ outcome: 'unsupported' });
    client.resumeLease = async () => ({ outcome: 'unsupported' });
    for (const call of [
      () => sandbox.snapshot(),
      () => sandbox.pause(),
      () => sandbox.resume(),
      () => Sandbox.deleteSnapshot(client, `sha256:${'a'.repeat(64)}` as never),
    ]) {
      const err = await call().catch((e: unknown) => e);
      expect(err).toBeInstanceOf(SandboxWsError);
      expect(err).toMatchObject({ code: 'unsupported' });
    }
  });

  it('pauses and resumes through the sandbox', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant, timeoutMs: 60_000 });
    expect(await sandbox.pause()).toBeInstanceOf(Date);
    expect(await sandbox.resume()).toBeInstanceOf(Date);
  });

  describe('provider-native snapshots', () => {
    const receipt = {
      reference: { provider: 'daytona' as const, id: 'trogon-abc', expires_at: 1_900_000_000 },
      bytes: 12,
    };

    it('asks for a native receipt only when told to and returns it', async () => {
      const client = new MemorySandboxClient();
      const sandbox = await Sandbox.create(client, { tenant, timeoutMs: 60_000 });
      const seen: Array<{ acceptNative?: boolean } | undefined> = [];
      client.snapshotLease = async (_id, opts) => {
        seen.push(opts);
        return { outcome: 'created_native', receipt };
      };
      expect(await sandbox.snapshot({ acceptNative: true })).toEqual(receipt);
      expect(seen).toEqual([{ acceptNative: true }]);
    });

    it('refuses a native receipt the call did not ask for', async () => {
      const client = new MemorySandboxClient();
      const sandbox = await Sandbox.create(client, { tenant, timeoutMs: 60_000 });
      client.snapshotLease = async () => ({ outcome: 'created_native', receipt });
      await expect(sandbox.snapshot()).rejects.toMatchObject({ code: 'unsupported' });
    });

    it('restores from a native receipt or reference as a native_snapshot image', async () => {
      const client = new MemorySandboxClient();
      const images: unknown[] = [];
      const lease = client.lease.bind(client);
      client.lease = async (request) => {
        images.push(request.spec.image);
        return lease({
          ...request,
          spec: { ...request.spec, image: { kind: 'registry', reference: { registry: '', repository: 'x' } } },
        });
      };
      await Sandbox.fromSnapshot(client, receipt, { tenant, timeoutMs: 60_000 });
      await Sandbox.create(client, { tenant, timeoutMs: 60_000, fromSnapshot: receipt.reference });
      expect(images).toEqual([
        { kind: 'native_snapshot', reference: receipt.reference },
        { kind: 'native_snapshot', reference: receipt.reference },
      ]);
    });
  });
});
