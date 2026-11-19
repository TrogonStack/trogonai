import { describe, expect, it } from 'vitest';

import { SandboxWsError } from '../src/errors.ts';
import { tenantId } from '../src/ids.ts';
import { MemorySandboxClient } from '../src/memory.ts';
import { allowListPolicy } from '../src/network.ts';
import { Sandbox } from '../src/sandbox.ts';
import type { LeaseRequest, NetworkPolicy } from '../src/wire.ts';

function recording(): { client: MemorySandboxClient; requests: LeaseRequest[] } {
  const client = new MemorySandboxClient();
  const requests: LeaseRequest[] = [];
  const lease = client.lease.bind(client);
  client.lease = async (request) => {
    requests.push(request);
    return lease(request);
  };
  return { client, requests };
}

const registries: NetworkPolicy = {
  user_defined: {
    rules: [
      { domain: 'registry.npmjs.org', action: 'pass_through' },
      { domain: '*.pythonhosted.org', action: 'pass_through' },
    ],
    allow_ranges: [],
    deny_ranges: [],
  },
};

describe('allowListPolicy', () => {
  it('builds a deny-by-default user_defined policy', () => {
    expect(allowListPolicy(['registry.npmjs.org', '*.pythonhosted.org'])).toEqual(registries);
  });

  it('trims and de-duplicates', () => {
    expect(allowListPolicy([' registry.npmjs.org ', '*.pythonhosted.org', 'registry.npmjs.org'])).toEqual(registries);
  });

  it('keeps ports', () => {
    expect(allowListPolicy(['example.com:443'])).toEqual({
      user_defined: {
        rules: [{ domain: 'example.com:443', action: 'pass_through' }],
        allow_ranges: [],
        deny_ranges: [],
      },
    });
  });

  it('maps an empty list to deny_all', () => {
    expect(allowListPolicy([])).toBe('deny_all');
  });

  it.each(['', '  ', 'a b', 'https://example.com', 'example.com/path'])('rejects %j', (entry) => {
    expect(() => allowListPolicy([entry])).toThrow(SandboxWsError);
  });
});

describe('network allow-list on Sandbox', () => {
  it('network: { allow } sets the spec policy', async () => {
    const { client, requests } = recording();
    await Sandbox.create(client, {
      tenant: tenantId('acme'),
      network: { allow: ['registry.npmjs.org', '*.pythonhosted.org'] },
    });
    expect(requests[0]?.spec.network_policy).toEqual(registries);
  });

  it('networkPolicy accepts the same shape', async () => {
    const { client, requests } = recording();
    await Sandbox.create(client, {
      tenant: tenantId('acme'),
      networkPolicy: { allow: ['registry.npmjs.org', '*.pythonhosted.org'] },
    });
    expect(requests[0]?.spec.network_policy).toEqual(registries);
  });

  it('still accepts the existing string and wire forms', async () => {
    const { client, requests } = recording();
    await Sandbox.create(client, { tenant: tenantId('acme'), networkPolicy: 'deny-all' });
    await Sandbox.create(client, { tenant: tenantId('acme'), networkPolicy: registries });
    expect(requests[0]?.spec.network_policy).toBe('deny_all');
    expect(requests[1]?.spec.network_policy).toEqual(registries);
  });

  it('rejects network together with networkPolicy', async () => {
    const { client, requests } = recording();
    await expect(
      Sandbox.create(client, { tenant: tenantId('acme'), network: { allow: ['a.com'] }, networkPolicy: 'deny-all' }),
    ).rejects.toThrow(/not both/);
    expect(requests).toEqual([]);
  });

  it('updateNetworkPolicy accepts an allow list', async () => {
    const client = new MemorySandboxClient();
    const sent: unknown[] = [];
    client.updateNetworkPolicy = async (_id, policy) => {
      sent.push(policy);
      return { outcome: 'applied' };
    };
    const sandbox = await Sandbox.create(client, { tenant: tenantId('acme') });
    await sandbox.updateNetworkPolicy({ allow: ['registry.npmjs.org', '*.pythonhosted.org'] });
    expect(sent).toEqual([registries]);
  });
});
