import { describe, expect, it } from 'vitest';

import { MemorySandboxClient, Sandbox, SandboxWsError, tenantId } from '../src/index.ts';
import type { CreateSandboxOptions, LeaseRequest, SandboxClientApi } from '../src/index.ts';

const tenant = tenantId('acme');
const browserImage = {
  kind: 'registry' as const,
  reference: { registry: 'registry.example', repository: 'browser', tag: 'stable' },
};

async function requestFor(
  options: Omit<CreateSandboxOptions, 'tenant'> & Record<string, unknown>,
): Promise<LeaseRequest> {
  let captured: LeaseRequest | undefined;
  const client = {
    lease: async (request: LeaseRequest) => {
      captured = request;
      throw new Error('request captured');
    },
  } as unknown as SandboxClientApi;
  await expect(Sandbox.create(client, { tenant, ...options })).rejects.toThrow('request captured');
  return captured!;
}

describe('Sandbox.create capability requirements', () => {
  it('forwards explicit requirements with the required wire defaults', async () => {
    const request = await requestFor({
      requiredCapabilities: { needs_terminal: true, needs_file_ops: true, min_egress_enforcement: 'mandatory' },
    });
    expect(request.required_capabilities).toEqual({
      needs_snapshot: false,
      needs_checkpoint_running: false,
      needs_restore: false,
      needs_warm_pool: false,
      needs_named_tags: false,
      needs_fuse_mounts: false,
      needs_terminal: true,
      needs_file_ops: true,
      min_egress_enforcement: 'mandatory',
    });
  });

  it('merges the browser shorthand without dropping other requirements or changing the explicit image', async () => {
    const request = await requestFor({
      browser: true,
      image: browserImage,
      requiredCapabilities: { needs_file_watch: true, needs_browser: false },
    });
    expect(request.required_capabilities).toMatchObject({ needs_browser: true, needs_file_watch: true });
    expect(request.spec.image).toEqual(browserImage);
  });

  it('refuses the browser shorthand without an explicit image before requesting a lease', async () => {
    const client = new MemorySandboxClient();
    let leased = false;
    client.lease = async () => {
      leased = true;
      throw new Error('should not lease');
    };
    const options = { tenant, browser: true };
    await expect(Sandbox.create(client, options)).rejects.toMatchObject({ code: 'invalid_request' });
    expect(leased).toBe(false);
  });

  it('accepts the browser image from the explicit spec', async () => {
    const request = await requestFor({
      browser: true,
      spec: { image: browserImage, workspace: { empty: {} }, env: {}, cpu_seconds: 60, memory_mib: 256 },
    });
    expect(request.required_capabilities?.needs_browser).toBe(true);
    expect(request.spec.image).toEqual(browserImage);
  });

  it('does not weaken an explicit browser requirement when the shorthand is false', async () => {
    const request = await requestFor({ browser: false, requiredCapabilities: { needs_browser: true } });
    expect(request.required_capabilities?.needs_browser).toBe(true);
  });

  it('omits requirements when neither the option nor shorthand requests them', async () => {
    expect(await requestFor({})).not.toHaveProperty('required_capabilities');
    expect(await requestFor({ browser: false })).not.toHaveProperty('required_capabilities');
  });

  it.each([
    { browser: 'yes' },
    { requiredCapabilities: { needs_terminal: 'yes' } },
    { requiredCapabilities: { needs_unknown_feature: true } },
    { requiredCapabilities: { vcpu_millis: -1 } },
    { requiredCapabilities: { exposed_port_count: 65536 } },
    { requiredCapabilities: { min_isolation_tier: 'unknown' } },
  ])('refuses malformed capability input before requesting a lease: %j', async (options) => {
    const client = new MemorySandboxClient();
    let leased = false;
    client.lease = async () => {
      leased = true;
      throw new Error('should not lease');
    };
    const pending = Sandbox.create(client, { tenant, ...options });
    await expect(pending).rejects.toBeInstanceOf(SandboxWsError);
    await expect(pending).rejects.toMatchObject({ code: 'invalid_request' });
    expect(leased).toBe(false);
  });

  it('the memory fake refuses a browser requirement rather than pretending to provide one', async () => {
    const options = { tenant, browser: true, image: browserImage };
    await expect(Sandbox.create(new MemorySandboxClient(), options)).rejects.toMatchObject({
      code: 'lease_denied',
      variant: 'no_worker_satisfies_capabilities',
    });
  });

  it.each(['needs_terminal', 'needs_fuse_mounts', 'needs_native_passthrough'])(
    'the memory fake refuses unsupported %s',
    async (requirement) => {
      const options = { tenant, requiredCapabilities: { [requirement]: true } };
      await expect(Sandbox.create(new MemorySandboxClient(), options)).rejects.toMatchObject({
        code: 'lease_denied',
        variant: 'no_worker_satisfies_capabilities',
      });
    },
  );

  it('the memory fake accepts an implemented snapshot requirement', async () => {
    const options = { tenant, requiredCapabilities: { needs_snapshot: true } };
    const sandbox = await Sandbox.create(new MemorySandboxClient(), options);
    expect((await sandbox.snapshot()).reference.registry).toBe('memory');
  });
});
