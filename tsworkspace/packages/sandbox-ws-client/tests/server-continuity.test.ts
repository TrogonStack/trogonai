import { afterEach, describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, SandboxWsError, tenantId } from '../src/index.ts';
import type { ArtifactType, NativeSnapshotRef, OciArtifactRef, ProviderKind, StartedServer } from '../src/index.ts';

const servers: StartedServer[] = [];
const endings: Array<() => void> = [];
afterEach(async () => {
  for (const server of servers.splice(0)) await server.stop();
  for (const end of endings.splice(0)) end();
});

async function fixture(referenceFor: (original: OciArtifactRef) => OciArtifactRef | NativeSnapshotRef, name?: string) {
  let starts = 0;
  const client = new MemorySandboxClient({
    commands: {
      serve: ({ signal }) => {
        starts += 1;
        return new Promise((resolve) => {
          const end = () => resolve({ exitCode: 143 });
          endings.push(end);
          signal.addEventListener('abort', end, { once: true });
        });
      },
    },
  });
  client.waitSandboxReady = async () => ({ outcome: 'ready' });
  const source = await Sandbox.create(client, { tenant: tenantId('acme') });
  servers.push(await source.startServer({ cmd: 'serve', port: 3000, preview: false, name }));
  const captured = JSON.parse(JSON.stringify(await source.snapshot()));
  const reference = referenceFor(captured.reference);
  const receipt = { ...captured, reference };
  const lease = client.lease.bind(client);
  client.lease = async (request) => ({
    ...(await lease({
      ...request,
      spec: { ...request.spec, image: { kind: 'snapshot', reference: captured.reference } },
    })),
    provider: 'provider' in reference ? reference.provider : 'mock',
  });
  client.listProcesses = async () => ({ outcome: 'unsupported' });
  return {
    client,
    captured,
    starts: () => starts,
    restore: () => Sandbox.fromSnapshot(client, receipt, { tenant: tenantId('acme') }),
    outcome: () =>
      Sandbox.fromSnapshot(client, receipt, { tenant: tenantId('acme') }).then(
        () => 'restored',
        (error: unknown) => (error instanceof SandboxWsError ? error.code : 'unexpected_error'),
      ),
  };
}

describe('snapshot process continuity', () => {
  it.each<ProviderKind>(['vercel', 'modal', 'fly'])(
    'restarts an unnamed server from a disk-only %s native snapshot',
    async (provider) => {
      const state = await fixture(() => ({ provider, id: 'disk-snapshot' }));
      expect(await state.outcome()).toBe('restored');
      expect(state.starts()).toBe(2);
    },
  );

  it.each<ArtifactType>(['sandbox_rootfs_v1', 'kubevirt_disk_v1'])(
    'restarts an unnamed server from the disk-only %s artifact',
    async (artifact_type) => {
      const state = await fixture((reference) => ({ ...reference, artifact_type }));
      expect(await state.outcome()).toBe('restored');
      expect(state.starts()).toBe(2);
    },
  );

  it.each<ArtifactType>(['firecracker_snapshot_v1', 'hyperlight_snapshot_v1'])(
    'does not assume a %s OCI artifact discarded its execution state',
    async (artifact_type) => {
      const state = await fixture((reference) => ({ ...reference, artifact_type }));
      expect(await state.outcome()).toBe('unsupported');
      expect(state.starts()).toBe(1);
    },
  );

  it('does not assume an unrecognized OCI format discarded its processes', async () => {
    const state = await fixture((reference) => ({ ...reference, artifact_type: 'future_snapshot_v2' as ArtifactType }));
    expect(await state.outcome()).toBe('unsupported');
    expect(state.starts()).toBe(1);
  });

  it.each<ProviderKind>(['e2b', 'mock'])(
    'keeps memory-preserving or unclassified %s native formats guarded',
    async (provider) => {
      const state = await fixture(() => ({ provider, id: 'state-snapshot' }));
      expect(await state.outcome()).toBe('unsupported');
      expect(state.starts()).toBe(1);
    },
  );

  it('reattaches a preserved unnamed OCI execution when authoritative discovery identifies it', async () => {
    const state = await fixture((reference) => ({ ...reference, artifact_type: 'firecracker_snapshot_v1' }));
    const execution = state.captured.clientState.servers[0].executionId as string;
    state.client.listProcesses = async () => ({
      outcome: 'processes',
      processes: [{ execution_id: execution, state: 'running' }],
    });
    const poll = state.client.pollCommand.bind(state.client);
    state.client.pollCommand = async (id, candidate) =>
      candidate === execution ? { outcome: 'running' } : poll(id, candidate);
    await state.restore();
    expect(state.starts()).toBe(1);
  });

  it('does not infer that a named memory-snapshot process ended merely because it is absent from discovery', async () => {
    const state = await fixture(() => ({ provider: 'e2b', id: 'state-snapshot' }), 'web');
    state.client.listProcesses = async () => ({ outcome: 'processes', processes: [] });
    expect(await state.outcome()).toBe('unsupported');
    expect(state.starts()).toBe(1);
  });

  it('restarts a memory-snapshot server when authoritative discovery confirms its original execution exited', async () => {
    const state = await fixture(() => ({ provider: 'e2b', id: 'state-snapshot' }));
    const execution = state.captured.clientState.servers[0].executionId as string;
    state.client.listProcesses = async () => ({
      outcome: 'processes',
      processes: [{ execution_id: execution, state: 'exited', exit_code: 1 }],
    });
    expect(await state.outcome()).toBe('restored');
    expect(state.starts()).toBe(2);
  });
});
