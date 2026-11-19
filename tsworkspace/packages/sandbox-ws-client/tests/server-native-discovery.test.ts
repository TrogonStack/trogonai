import { afterEach, describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, SandboxWsError, tenantId } from '../src/index.ts';
import type { ListProcessesResponse, StartedServer } from '../src/index.ts';

const servers: StartedServer[] = [];
const endings: Array<() => void> = [];
afterEach(async () => {
  for (const server of servers.splice(0)) await server.stop();
  for (const end of endings.splice(0)) end();
});

async function nativeRestore(name?: string) {
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
  const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
  servers.push(await sandbox.startServer({ cmd: 'serve', port: 3000, preview: false, name }));
  const captured = JSON.parse(JSON.stringify(await sandbox.snapshot()));
  const receipt = {
    reference: { provider: 'e2b' as const, id: 'native-copy' },
    bytes: 0,
    clientState: captured.clientState,
  };
  const lease = client.lease.bind(client);
  client.lease = async (request) => ({
    ...(await lease({
      ...request,
      spec: { ...request.spec, image: { kind: 'snapshot', reference: captured.reference } },
    })),
    provider: 'e2b',
  });
  return {
    client,
    starts: () => starts,
    outcome: () =>
      Sandbox.fromSnapshot(client, receipt, { tenant: tenantId('acme') }).then(
        () => 'restored',
        (error: unknown) => (error instanceof SandboxWsError ? error.code : 'unexpected_error'),
      ),
  };
}

describe('native server process discovery', () => {
  it.each<ListProcessesResponse>([
    { outcome: 'processes', processes: [] },
    { outcome: 'processes', processes: [{ execution_id: 'unknown-preserved-id', state: 'running' }] },
  ])('does not relaunch an unnamed server whose preserved process identity is unknown', async (inventory) => {
    const fixture = await nativeRestore();
    fixture.client.listProcesses = async () => inventory;
    expect(await fixture.outcome()).toBe('unsupported');
    expect(fixture.starts()).toBe(1);
  });

  it('refuses unsupported discovery without launching a duplicate', async () => {
    const fixture = await nativeRestore('web');
    fixture.client.listProcesses = async () => ({ outcome: 'unsupported' });
    expect(await fixture.outcome()).toBe('unsupported');
    expect(fixture.starts()).toBe(1);
  });

  it('refuses an ambiguous named process inventory', async () => {
    const fixture = await nativeRestore('web');
    fixture.client.listProcesses = async () => ({
      outcome: 'processes',
      processes: [
        { execution_id: 'first-web', name: 'web', state: 'running' },
        { execution_id: 'second-web', name: 'web', state: 'running' },
      ],
    });
    expect(await fixture.outcome()).toBe('invalid_response');
    expect(fixture.starts()).toBe(1);
  });

  it('refuses malformed process data without inferring that the server is gone', async () => {
    const fixture = await nativeRestore('web');
    fixture.client.listProcesses = async () =>
      ({
        outcome: 'processes',
        processes: [{ execution_id: 'unknown', state: 'unrecognized' }],
      }) as unknown as ListProcessesResponse;
    expect(await fixture.outcome()).toBe('invalid_response');
    expect(fixture.starts()).toBe(1);
  });
});
