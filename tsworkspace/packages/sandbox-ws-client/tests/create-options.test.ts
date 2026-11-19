import { describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, SandboxWsError, tenantId } from '../src/index.ts';
import type { CreateSandboxOptions, LeaseRequest, SecretBinding } from '../src/index.ts';

const tenant = tenantId('acme');

const binding: SecretBinding = {
  name: 'github',
  target: { env: 'GITHUB_TOKEN' },
  mode: 'mask',
  inject_hosts: ['api.github.com'],
  source: {
    provider: 'registry',
    pull_secret_id: 'ps-1' as never,
    scope: { kind: 'registry_pull', repository_glob: '*' },
    ttl_seconds: 300,
  },
};

async function createdRequest(opts: Omit<CreateSandboxOptions, 'tenant'>): Promise<LeaseRequest> {
  const client = new MemorySandboxClient();
  let seen: LeaseRequest | undefined;
  const lease = client.lease.bind(client);
  client.lease = (async (request) => {
    seen = request;
    return lease(request);
  }) as typeof client.lease;
  await Sandbox.create(client, { tenant, ...opts });
  return seen!;
}

describe('typed ports and secrets on create', () => {
  it('maps ports to exposed_ports and secrets to secret_bindings', async () => {
    const request = await createdRequest({ ports: [3000, 8080], secrets: [binding] });
    expect(request.spec.exposed_ports).toEqual([3000, 8080]);
    expect(request.spec.secret_bindings).toEqual([binding]);
  });

  it('leaves both fields off the wire when not given', async () => {
    const request = await createdRequest({});
    expect(request.spec).not.toHaveProperty('exposed_ports');
    expect(request.spec).not.toHaveProperty('secret_bindings');
  });

  it('overrides the fields of a spec escape hatch without mutating the caller spec', async () => {
    const client = new MemorySandboxClient();
    const spec = {
      image: {
        kind: 'registry' as const,
        reference: { registry: 'docker.io', repository: 'library/alpine', tag: 'latest' },
      },
      workspace: { empty: {} },
      env: {},
      cpu_seconds: 60,
      memory_mib: 256,
      exposed_ports: [1111],
    };
    await Sandbox.create(client, { tenant, spec, ports: [2222] });
    expect(spec.exposed_ports).toEqual([1111]);
  });

  it.each([
    ['a port of zero', { ports: [0] }, /ports/],
    ['a port above 65535', { ports: [65536] }, /ports/],
    ['a fractional port', { ports: [80.5] }, /ports/],
    ['duplicate ports', { ports: [80, 80] }, /distinct/],
    ['more than 15 ports', { ports: Array.from({ length: 16 }, (_, i) => 3000 + i) }, /ports/],
    ['an empty secret name', { secrets: [{ ...binding, name: '' }] }, /secrets\.0\.name/],
    ['a bad env var name', { secrets: [{ ...binding, target: { env: '1BAD' } }] }, /secrets\.0\.target/],
    ['a relative file target', { secrets: [{ ...binding, target: { file: 'rel/path' } }] }, /secrets\.0\.target/],
    ['no inject hosts', { secrets: [{ ...binding, inject_hosts: [] }] }, /inject_hosts/],
    ['a bad mode', { secrets: [{ ...binding, mode: 'leak' as never }] }, /mode/],
    [
      'duplicate secret names',
      { secrets: [binding, { ...binding, target: { env: 'OTHER' } }] },
      /duplicate secret name/,
    ],
    ['duplicate secret targets', { secrets: [binding, { ...binding, name: 'second' }] }, /duplicate secret target/],
  ])('rejects %s before leasing', async (_label, opts, message) => {
    const client = new MemorySandboxClient();
    let leased = false;
    client.lease = (async () => {
      leased = true;
      throw new Error('should not lease');
    }) as typeof client.lease;
    const attempt = Sandbox.create(client, { tenant, ...(opts as Omit<CreateSandboxOptions, 'tenant'>) });
    await expect(attempt).rejects.toBeInstanceOf(SandboxWsError);
    await expect(attempt).rejects.toThrow(message);
    expect(leased).toBe(false);
  });

  it('rejects an env secret that env already sets', async () => {
    await expect(
      Sandbox.create(new MemorySandboxClient(), { tenant, env: { GITHUB_TOKEN: 'x' }, secrets: [binding] }),
    ).rejects.toThrow(/already sets/);
  });
});

describe('region and native passthrough on create', () => {
  it('maps region, fallbackRegions and native onto the wire request', async () => {
    const client = new MemorySandboxClient();
    let seen: LeaseRequest | undefined;
    client.lease = (async (request) => {
      seen = request;
      throw new SandboxWsError('no_worker_in_region', 'stop here');
    }) as typeof client.lease;
    await expect(
      Sandbox.create(client, {
        tenant,
        region: 'eu',
        fallbackRegions: ['us'],
        native: { daytona: { auto_stop_interval: 30 } },
      }),
    ).rejects.toThrow();
    expect(seen?.region).toBe('eu');
    expect(seen?.fallback_regions).toEqual(['us']);
    expect(seen?.native).toEqual({ daytona: { auto_stop_interval: 30 } });
  });

  it('leaves the fields off the wire when not given', async () => {
    const request = await createdRequest({});
    expect(request).not.toHaveProperty('region');
    expect(request).not.toHaveProperty('native');
    expect(request).not.toHaveProperty('fallback_regions');
  });

  it('the in-memory fake refuses native and region with typed errors', async () => {
    const client = new MemorySandboxClient();
    await expect(Sandbox.create(client, { tenant, native: { mock: {} } })).rejects.toMatchObject({
      code: 'native_passthrough_not_permitted',
    });
    await expect(Sandbox.create(client, { tenant, region: 'eu' })).rejects.toMatchObject({
      code: 'no_worker_in_region',
    });
  });

  it('the in-memory fake has no native handle', async () => {
    const client = new MemorySandboxClient();
    const sandbox = await Sandbox.create(client, { tenant });
    await expect(sandbox.nativeHandle()).rejects.toThrow(/no native handle/);
    await expect(sandbox.sshAccess()).rejects.toThrow(/no ssh access/);
  });
});
